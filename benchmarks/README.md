# TARON CoolMine Benchmarking Framework

This directory contains the complete benchmarking framework for comparing TARON's CoolMine algorithm with other cryptocurrency mining implementations.

## Files Overview

### 📊 `energy-comparison.md`
Comprehensive energy efficiency comparison between TARON CoolMine and major cryptocurrencies:
- Bitcoin (SHA-256, ASIC)
- Ethereum (historical PoW Ethash)  
- Monero (RandomX, CPU)
- TARON (SEQUAL-256 CoolMine, CPU)

Includes detailed methodology, power measurements, and environmental impact analysis.

### 🔬 `run_benchmark.sh`
Automated benchmark script that:
- Compiles TARON in release mode
- Runs 60-second mining benchmark
- Measures hashrate, CPU temperature, frequency, load average
- Collects power consumption data via Intel RAPL (if available)
- Outputs clean JSON results with timestamps

### 📖 `../docs/coolmine-whitepaper-section.md`
Technical whitepaper section explaining:
- Thermal-adaptive mining architecture
- Idle-burst pattern implementation
- Security properties and consensus compatibility
- Performance benchmarks and environmental impact

## Quick Start

### Run Basic Benchmark
```bash
cd /path/to/taron
sudo ./benchmarks/run_benchmark.sh
```

### Custom Benchmark Duration
```bash
sudo ./benchmarks/run_benchmark.sh --duration 120 --output my_benchmark.json
```

### View Results
```bash
jq . benchmark_results_20260302_072745.json
```

## Requirements

### System Requirements
- Linux kernel 4.14+ (for thermal monitoring)
- Rust toolchain (for compilation)
- Root privileges (for power measurements)
- Intel CPU with RAPL support (recommended)

### Software Dependencies
- `cargo` (Rust build system)
- `jq` (JSON processing)
- `bc` (arithmetic calculations)

### Install Dependencies (Ubuntu/Debian)
```bash
sudo apt update
sudo apt install jq bc cargo
```

## Benchmark Output Format

The script generates JSON output with the following structure:

```json
{
    "benchmark_info": {
        "version": "1.0",
        "timestamp": "2026-03-02T07:27:45Z",
        "duration_seconds": 60,
        "sample_interval_seconds": 1,
        "samples_collected": 60
    },
    "system_info": {
        "cpu": {
            "model": "Intel(R) Core(TM) i7-12700K",
            "cores": 8,
            "threads": 16
        },
        "os": "Linux 6.12.73",
        "taron_binary": "./target/release/taron"
    },
    "performance_metrics": {
        "hashrate_hs": 6420.5,
        "average_temperature_celsius": 67.2,
        "average_frequency_mhz": 3600,
        "average_load": {
            "load1": 4.2,
            "load5": 2.8,
            "load15": 1.9
        },
        "power_consumption_watts": 8.7
    },
    "energy_efficiency": {
        "joules_per_hash": 0.00135,
        "watts_per_hash": 0.00000135
    },
    "mining_output": "[raw mining process output]"
}
```

## Power Measurement

### Intel RAPL Support
For accurate power measurements, ensure:
1. Running as root: `sudo ./run_benchmark.sh`
2. Intel CPU with RAPL support
3. `intel-rapl` kernel module loaded: `lsmod | grep rapl`

### Alternative Power Measurement
If RAPL is unavailable, consider:
- External power meters (Kill-a-Watt, smart plugs)
- UPS monitoring (if mining rig connected to UPS)
- Motherboard sensors (via `sensors` command)

## Interpreting Results

### Key Metrics
- **Hashrate**: Higher is better (more computation per second)
- **Temperature**: Should remain below 80°C for optimal efficiency
- **Power Consumption**: Lower is better (energy efficiency)
- **Watts per Hash**: Primary efficiency metric - lower values indicate more efficient mining

### CoolMine Optimization
Optimal CoolMine performance characteristics:
- Temperature cycling between 65-75°C
- Power consumption 8-12W average
- Hashrate 6,000-7,000 H/s (depending on hardware)
- Efficient thermal management without throttling

## Troubleshooting

### Common Issues

**Permission Denied (Power Monitoring)**
```bash
# Solution: Run with sudo
sudo ./run_benchmark.sh
```

**Missing Dependencies**
```bash
# Install required tools
sudo apt install jq bc
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

**Temperature Sensors Not Found**
```bash
# Check available sensors
ls /sys/class/hwmon/*/temp*_input

# Install lm-sensors if needed
sudo apt install lm-sensors
sudo sensors-detect
```

**Compilation Failures**
```bash
# Update Rust toolchain
rustup update stable

# Clean and rebuild
cargo clean
cargo build --release
```

### Advanced Configuration

Modify script variables at the top of `run_benchmark.sh`:
```bash
BENCHMARK_DURATION=60    # Test duration in seconds
SAMPLE_INTERVAL=1        # Measurement frequency
```

## Integration with CI/CD

### Automated Testing
```bash
#!/bin/bash
# ci-benchmark.sh
cd taron/
./benchmarks/run_benchmark.sh --duration 30 --output ci_results.json

# Verify minimum performance thresholds
HASHRATE=$(jq -r '.performance_metrics.hashrate_hs' ci_results.json)
if (( $(echo "$HASHRATE < 5000" | bc -l) )); then
    echo "Performance regression detected"
    exit 1
fi
```

### Performance Tracking
Store benchmark results in time-series database for regression analysis:
```bash
# Example: Store results in InfluxDB
TIMESTAMP=$(jq -r '.benchmark_info.timestamp' results.json)
HASHRATE=$(jq -r '.performance_metrics.hashrate_hs' results.json)
POWER=$(jq -r '.performance_metrics.power_consumption_watts' results.json)

curl -X POST 'http://influxdb:8086/write?db=taron_benchmarks' \
  --data-binary "performance,host=$HOSTNAME hashrate=$HASHRATE,power=$POWER $TIMESTAMP"
```

## Contributing

### Adding New Metrics
To add additional measurements, modify the monitoring loop in `run_mining_benchmark()`:

```bash
# Example: Add memory usage monitoring
get_memory_usage() {
    awk '/MemAvailable:/ {print int($2/1024)}' /proc/meminfo
}

# Add to monitoring loop
local memory=$(get_memory_usage)
memory_readings+=("$memory")
```

### Supporting New Platforms
- AMD CPU power monitoring via `amd_energy` driver
- ARM processor support via `thermal_zone` interfaces
- Windows PowerShell version for cross-platform compatibility

## License

This benchmarking framework is released under the same license as TARON (MIT). See the main repository LICENSE file for details.

---

**Version**: 1.0  
**Last Updated**: March 2026  
**Maintainer**: TARON Core Development Team