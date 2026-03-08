#!/bin/bash

# TARON CoolMine Benchmarking Script
# Measures mining performance, energy consumption, and thermal characteristics
# Author: TARON DEVOPS Team
# Version: 1.0

set -euo pipefail

# Configuration
BENCHMARK_DURATION=60
SAMPLE_INTERVAL=1
OUTPUT_FILE="benchmark_results_$(date +%Y%m%d_%H%M%S).json"
TARON_BINARY=""
TEMP_DIR="/tmp/taron_benchmark_$$"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Logging function
log() {
    echo -e "${BLUE}[$(date '+%H:%M:%S')]${NC} $1"
}

error() {
    echo -e "${RED}[ERROR]${NC} $1" >&2
    exit 1
}

warn() {
    echo -e "${YELLOW}[WARNING]${NC} $1" >&2
}

success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

# Cleanup function
cleanup() {
    log "Cleaning up temporary files..."
    rm -rf "$TEMP_DIR" 2>/dev/null || true
    
    # Kill any remaining taron processes
    pkill -f "taron.*mine" 2>/dev/null || true
    
    # Reset CPU frequency scaling if modified
    if [ -f /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor ]; then
        echo "powersave" | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor >/dev/null 2>&1 || true
    fi
}

trap cleanup EXIT INT TERM

# Check system requirements
check_requirements() {
    log "Checking system requirements..."
    
    # Check if running as root for power measurements
    if [ "$EUID" -ne 0 ] && [ ! -r /sys/class/powercap/intel-rapl ]; then
        warn "Not running as root - power measurements may be limited"
    fi
    
    # Check required tools
    local missing_tools=()
    
    command -v cargo >/dev/null || missing_tools+=("cargo")
    command -v jq >/dev/null || missing_tools+=("jq")
    command -v bc >/dev/null || missing_tools+=("bc")
    
    if [ ${#missing_tools[@]} -gt 0 ]; then
        error "Missing required tools: ${missing_tools[*]}"
    fi
    
    # Check CPU temperature monitoring
    if [ ! -d /sys/class/hwmon ]; then
        warn "Hardware monitoring not available - temperature readings disabled"
    fi
    
    success "System requirements satisfied"
}

# Compile TARON in release mode
compile_taron() {
    log "Compiling TARON in release mode..."
    
    cd "$(dirname "$0")/.." || error "Failed to change directory"
    
    # Clean previous builds
    cargo clean >/dev/null 2>&1 || true
    
    # Build with optimizations
    log "Building release binary (this may take a few minutes)..."
    if ! cargo build --release --quiet 2>/dev/null; then
        error "Failed to compile TARON - check your Rust installation and dependencies"
    fi
    
    # Find the compiled binary
    TARON_BINARY="$(find target/release -name "taron*" -type f -executable | head -1)"
    
    if [ -z "$TARON_BINARY" ] || [ ! -x "$TARON_BINARY" ]; then
        error "Could not find compiled TARON binary in target/release/"
    fi
    
    success "TARON compiled successfully: $TARON_BINARY"
}

# Get CPU information
get_cpu_info() {
    local cpu_model=$(grep "model name" /proc/cpuinfo | head -1 | cut -d: -f2 | sed 's/^ *//')
    local cpu_cores=$(nproc)
    local cpu_threads=$(grep "processor" /proc/cpuinfo | wc -l)
    
    echo "{\"model\": \"$cpu_model\", \"cores\": $cpu_cores, \"threads\": $cpu_threads}"
}

# Get current CPU temperature (Celsius)
get_cpu_temp() {
    local temp_file
    local temp_value=0
    local temp_count=0
    
    # Try different temperature sensors
    for temp_file in /sys/class/hwmon/hwmon*/temp*_input; do
        if [ -r "$temp_file" ]; then
            local temp=$(cat "$temp_file" 2>/dev/null || echo "0")
            if [ "$temp" -gt 10000 ]; then  # Reasonable temperature range (>10°C)
                temp=$((temp / 1000))
                temp_value=$((temp_value + temp))
                temp_count=$((temp_count + 1))
            fi
        fi
    done
    
    if [ $temp_count -gt 0 ]; then
        echo $((temp_value / temp_count))
    else
        echo "null"
    fi
}

# Get current CPU frequency (MHz)
get_cpu_freq() {
    local freq_sum=0
    local freq_count=0
    local freq_file
    
    for freq_file in /sys/devices/system/cpu/cpu*/cpufreq/scaling_cur_freq; do
        if [ -r "$freq_file" ]; then
            local freq=$(cat "$freq_file" 2>/dev/null || echo "0")
            if [ "$freq" -gt 0 ]; then
                freq_sum=$((freq_sum + freq))
                freq_count=$((freq_count + 1))
            fi
        fi
    done
    
    if [ $freq_count -gt 0 ]; then
        echo $((freq_sum / freq_count / 1000))  # Convert to MHz
    else
        # Fallback to /proc/cpuinfo
        grep "cpu MHz" /proc/cpuinfo | head -1 | awk '{print int($4)}' || echo "null"
    fi
}

# Get system load average
get_load_average() {
    awk '{print $1, $2, $3}' /proc/loadavg | jq -R 'split(" ") | {load1: (.[0] | tonumber), load5: (.[1] | tonumber), load15: (.[2] | tonumber)}'
}

# Get power consumption via Intel RAPL
get_power_consumption() {
    local rapl_path="/sys/class/powercap/intel-rapl"
    
    if [ ! -d "$rapl_path" ]; then
        echo "null"
        return
    fi
    
    # Try to read CPU package power
    local energy_files=$(find "$rapl_path" -name "energy_uj" -path "*/intel-rapl:0/*" 2>/dev/null | head -1)
    
    if [ -n "$energy_files" ] && [ -r "$energy_files" ]; then
        cat "$energy_files" 2>/dev/null || echo "null"
    else
        echo "null"
    fi
}

# Calculate power consumption difference
calculate_power_watts() {
    local energy_start=$1
    local energy_end=$2
    local time_seconds=$3
    
    if [ "$energy_start" = "null" ] || [ "$energy_end" = "null" ] || [ $time_seconds -eq 0 ]; then
        echo "null"
        return
    fi
    
    local energy_diff=$((energy_end - energy_start))
    local power_watts=$(echo "scale=2; $energy_diff / $time_seconds / 1000000" | bc)
    echo "$power_watts"
}

# Parse hashrate from TARON mining output
parse_hashrate() {
    local mining_output="$1"
    
    # Extract the last hashrate reading from output
    local hashrate=$(echo "$mining_output" | grep -i "hash" | tail -1 | grep -o '[0-9.]\+ [KMG]\?H/s' | head -1)
    
    if [ -n "$hashrate" ]; then
        # Convert to standard H/s format
        local value=$(echo "$hashrate" | awk '{print $1}')
        local unit=$(echo "$hashrate" | awk '{print $2}')
        
        case "$unit" in
            "KH/s"|"kH/s") value=$(echo "$value * 1000" | bc) ;;
            "MH/s"|"mH/s") value=$(echo "$value * 1000000" | bc) ;;
            "GH/s"|"gH/s") value=$(echo "$value * 1000000000" | bc) ;;
        esac
        
        echo "$value"
    else
        echo "null"
    fi
}

# Run mining benchmark
run_mining_benchmark() {
    log "Starting mining benchmark for $BENCHMARK_DURATION seconds..."
    
    mkdir -p "$TEMP_DIR"
    
    # Initialize measurement arrays
    local timestamps=()
    local temperatures=()
    local frequencies=()
    local loads=()
    local power_readings=()
    
    # Get initial power reading
    local power_start=$(get_power_consumption)
    local start_time=$(date +%s)
    
    # Start mining process in background
    log "Launching TARON mining process..."
    local mining_output_file="$TEMP_DIR/mining_output.log"
    local mining_pid
    
    # Start mining (adapt command based on actual TARON CLI interface)
    "$TARON_BINARY" mine --benchmark --duration="$BENCHMARK_DURATION" > "$mining_output_file" 2>&1 &
    mining_pid=$!
    
    log "Mining PID: $mining_pid"
    
    # Monitor system metrics during mining
    local sample_count=0
    local end_time=$((start_time + BENCHMARK_DURATION))
    
    while [ $(date +%s) -lt $end_time ] && kill -0 $mining_pid 2>/dev/null; do
        local current_time=$(date +%s)
        local temp=$(get_cpu_temp)
        local freq=$(get_cpu_freq)
        local load=$(get_load_average)
        local power=$(get_power_consumption)
        
        timestamps+=("$current_time")
        temperatures+=("$temp")
        frequencies+=("$freq")
        loads+=("$load")
        power_readings+=("$power")
        
        sample_count=$((sample_count + 1))
        
        # Progress indicator
        local progress=$(( (current_time - start_time) * 100 / BENCHMARK_DURATION ))
        printf "\r${BLUE}Progress: %d%% | Temp: %s°C | Freq: %s MHz | Load: %s${NC}" \
               "$progress" "$temp" "$freq" "$(echo "$load" | jq -r '.load1')" 2>/dev/null || true
        
        sleep $SAMPLE_INTERVAL
    done
    
    echo  # New line after progress indicator
    
    # Wait for mining to complete and get final power reading
    wait $mining_pid 2>/dev/null || true
    local actual_end_time=$(date +%s)
    local power_end=$(get_power_consumption)
    
    # Read mining output
    local mining_output=""
    if [ -f "$mining_output_file" ]; then
        mining_output=$(cat "$mining_output_file")
    fi
    
    success "Mining benchmark completed ($((actual_end_time - start_time)) seconds)"
    
    # Parse results
    local hashrate=$(parse_hashrate "$mining_output")
    local avg_temp=$(echo "${temperatures[@]}" | tr ' ' '\n' | awk '$1 != "null" {sum+=$1; count++} END {if(count>0) print sum/count; else print "null"}')
    local avg_freq=$(echo "${frequencies[@]}" | tr ' ' '\n' | awk '$1 != "null" {sum+=$1; count++} END {if(count>0) print int(sum/count); else print "null"}')
    local avg_load=$(echo "${loads[@]}" | tr ' ' '\n' | jq -s 'map(select(. != "null")) | if length > 0 then {load1: (map(.load1) | add / length), load5: (map(.load5) | add / length), load15: (map(.load15) | add / length)} else null end' 2>/dev/null || echo "null")
    local power_watts=$(calculate_power_watts "$power_start" "$power_end" "$((actual_end_time - start_time))")
    
    # Create results JSON
    cat > "$TEMP_DIR/results.json" << EOF
{
    "benchmark_info": {
        "version": "1.0",
        "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
        "duration_seconds": $((actual_end_time - start_time)),
        "sample_interval_seconds": $SAMPLE_INTERVAL,
        "samples_collected": $sample_count
    },
    "system_info": {
        "cpu": $(get_cpu_info),
        "os": "$(uname -s) $(uname -r)",
        "taron_binary": "$TARON_BINARY"
    },
    "performance_metrics": {
        "hashrate_hs": $hashrate,
        "average_temperature_celsius": $avg_temp,
        "average_frequency_mhz": $avg_freq,
        "average_load": $avg_load,
        "power_consumption_watts": $power_watts
    },
    "energy_efficiency": {
        "joules_per_hash": $(if [ "$hashrate" != "null" ] && [ "$power_watts" != "null" ]; then echo "scale=10; $power_watts / $hashrate" | bc; else echo "null"; fi),
        "watts_per_hash": $(if [ "$hashrate" != "null" ] && [ "$power_watts" != "null" ]; then echo "scale=15; $power_watts / $hashrate" | bc; else echo "null"; fi)
    },
    "mining_output": $(echo "$mining_output" | jq -Rs .)
}
EOF
    
    # Copy results to final location
    cp "$TEMP_DIR/results.json" "$OUTPUT_FILE"
    
    success "Results saved to: $OUTPUT_FILE"
}

# Display results summary
display_summary() {
    log "Benchmark Summary:"
    echo
    
    if [ ! -f "$OUTPUT_FILE" ]; then
        error "Results file not found: $OUTPUT_FILE"
    fi
    
    # Extract key metrics using jq
    local hashrate=$(jq -r '.performance_metrics.hashrate_hs // "N/A"' "$OUTPUT_FILE")
    local temp=$(jq -r '.performance_metrics.average_temperature_celsius // "N/A"' "$OUTPUT_FILE")
    local freq=$(jq -r '.performance_metrics.average_frequency_mhz // "N/A"' "$OUTPUT_FILE")
    local power=$(jq -r '.performance_metrics.power_consumption_watts // "N/A"' "$OUTPUT_FILE")
    local efficiency=$(jq -r '.energy_efficiency.watts_per_hash // "N/A"' "$OUTPUT_FILE")
    
    echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
    echo -e "${GREEN}            TARON CoolMine Benchmark Results        ${NC}"
    echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
    echo
    echo -e "  ${BLUE}Hash Rate:${NC}           $hashrate H/s"
    echo -e "  ${BLUE}Average Temperature:${NC} $temp°C"
    echo -e "  ${BLUE}Average Frequency:${NC}   $freq MHz"
    echo -e "  ${BLUE}Power Consumption:${NC}   $power W"
    echo -e "  ${BLUE}Energy Efficiency:${NC}   $efficiency W/Hash"
    echo
    echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
    echo
    echo "Full results available in: $OUTPUT_FILE"
    echo "View with: jq . $OUTPUT_FILE"
}

# Main execution
main() {
    echo -e "${GREEN}"
    echo "████████╗ █████╗ ██████╗  ██████╗ ███╗   ██╗"
    echo "╚══██╔══╝██╔══██╗██╔══██╗██╔═══██╗████╗  ██║"
    echo "   ██║   ███████║██████╔╝██║   ██║██╔██╗ ██║"
    echo "   ██║   ██╔══██║██╔══██╗██║   ██║██║╚██╗██║"
    echo "   ██║   ██║  ██║██║  ██║╚██████╔╝██║ ╚████║"
    echo "   ╚═╝   ╚═╝  ╚═╝╚═╝  ╚═╝ ╚═════╝ ╚═╝  ╚═══╝"
    echo -e "${NC}"
    echo "            CoolMine Benchmark Suite v1.0"
    echo
    
    check_requirements
    compile_taron
    run_mining_benchmark
    display_summary
    
    success "Benchmark completed successfully!"
}

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        -d|--duration)
            BENCHMARK_DURATION="$2"
            shift 2
            ;;
        -i|--interval)
            SAMPLE_INTERVAL="$2"
            shift 2
            ;;
        -o|--output)
            OUTPUT_FILE="$2"
            shift 2
            ;;
        -h|--help)
            echo "TARON CoolMine Benchmark Suite"
            echo
            echo "Usage: $0 [OPTIONS]"
            echo
            echo "Options:"
            echo "  -d, --duration SECONDS   Benchmark duration in seconds (default: 60)"
            echo "  -i, --interval SECONDS   Sampling interval in seconds (default: 1)"
            echo "  -o, --output FILE        Output file name (default: auto-generated)"
            echo "  -h, --help              Show this help message"
            echo
            exit 0
            ;;
        *)
            error "Unknown option: $1. Use -h for help."
            ;;
    esac
done

# Run the main function
main "$@"