//! Thermal Governor for CoolMine
//!
//! Monitors CPU temperature via Linux thermal zones and automatically scales
//! mining intensity to maintain target temperature (default 55°C).
//!
//! Uses PID control loop for smooth adjustment without thermal oscillation.

use parking_lot::RwLock;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Target CPU temperature in millicelsius (55°C = 55000)
const DEFAULT_TARGET_TEMP: i32 = 55_000;

/// Critical temperature threshold - emergency throttling (85°C = 85000)
const CRITICAL_TEMP: i32 = 85_000;

/// PID controller constants for smooth thermal control
const KP: f32 = 0.8; // Proportional gain
const KI: f32 = 0.1; // Integral gain  
const KD: f32 = 0.2; // Derivative gain

/// Minimum intensity (10% - never completely stop)
const MIN_INTENSITY: f32 = 0.1;

/// Maximum intensity (100% - full throttle)
const MAX_INTENSITY: f32 = 1.0;

/// CPU thermal controller state
#[derive(Debug, Clone)]
pub struct ThermalState {
    pub current_temp: i32,
    pub target_temp: i32,
    pub intensity: f32,
    pub is_critical: bool,
    pub last_update: Instant,
}

impl Default for ThermalState {
    fn default() -> Self {
        Self {
            current_temp: 0,
            target_temp: DEFAULT_TARGET_TEMP,
            intensity: 1.0,
            is_critical: false,
            last_update: Instant::now(),
        }
    }
}

/// PID controller for thermal management
#[derive(Debug)]
struct PidController {
    previous_error: f32,
    integral: f32,
    last_time: Instant,
}

impl PidController {
    fn new() -> Self {
        Self {
            previous_error: 0.0,
            integral: 0.0,
            last_time: Instant::now(),
        }
    }

    /// Compute PID output for thermal control
    fn compute(&mut self, error: f32) -> f32 {
        let now = Instant::now();
        let dt = now.duration_since(self.last_time).as_secs_f32();
        
        if dt < 0.001 {
            return 0.0; // Avoid division by zero for very fast calls
        }

        // Proportional term
        let proportional = KP * error;

        // Integral term (accumulated error over time)
        self.integral += error * dt;
        self.integral = self.integral.clamp(-10.0, 10.0); // Prevent windup
        let integral = KI * self.integral;

        // Derivative term (rate of change of error)
        let derivative = KD * (error - self.previous_error) / dt;

        self.previous_error = error;
        self.last_time = now;

        proportional + integral + derivative
    }
}

/// Thermal governor for mining intensity control
pub struct ThermalGovernor {
    state: Arc<RwLock<ThermalState>>,
    pid: RwLock<PidController>,
    thermal_zones: Vec<String>,
}

impl ThermalGovernor {
    /// Create new thermal governor with discovered thermal zones
    pub fn new() -> Self {
        let thermal_zones = Self::discover_thermal_zones();
        
        info!(
            "ThermalGovernor initialized with {} thermal zones",
            thermal_zones.len()
        );
        
        Self {
            state: Arc::new(RwLock::new(ThermalState::default())),
            pid: RwLock::new(PidController::new()),
            thermal_zones,
        }
    }

    /// Create thermal governor with custom target temperature
    pub fn with_target_temp(target_temp_celsius: f32) -> Self {
        let governor = Self::new();
        governor.state.write().target_temp = (target_temp_celsius * 1000.0) as i32;
        governor
    }

    /// Discover available thermal zones on the system
    fn discover_thermal_zones() -> Vec<String> {
        let mut zones = Vec::new();
        
        // Check common thermal zone paths
        for i in 0..32 {
            let path = format!("/sys/class/thermal/thermal_zone{}/temp", i);
            if Path::new(&path).exists() {
                zones.push(path);
            }
        }

        if zones.is_empty() {
            warn!("No thermal zones found - thermal control disabled");
        } else {
            debug!("Discovered thermal zones: {:?}", zones);
        }

        zones
    }

    /// Read current CPU temperature from thermal zones
    fn read_temperature(&self) -> Option<i32> {
        if self.thermal_zones.is_empty() {
            return None;
        }

        let mut total_temp = 0i32;
        let mut valid_readings = 0;

        for zone_path in &self.thermal_zones {
            match fs::read_to_string(zone_path) {
                Ok(content) => {
                    if let Ok(temp) = content.trim().parse::<i32>() {
                        // Sanity check: temperature should be reasonable (0-120°C)
                        if temp > 0 && temp < 120_000 {
                            total_temp += temp;
                            valid_readings += 1;
                        }
                    }
                }
                Err(e) => {
                    debug!("Failed to read thermal zone {}: {}", zone_path, e);
                }
            }
        }

        if valid_readings > 0 {
            Some(total_temp / valid_readings) // Average temperature
        } else {
            warn!("No valid temperature readings available");
            None
        }
    }

    /// Update thermal state and compute new mining intensity
    pub fn update(&self) {
        let current_temp = match self.read_temperature() {
            Some(temp) => temp,
            None => {
                // No thermal data - run at full intensity
                let mut state = self.state.write();
                state.intensity = MAX_INTENSITY;
                state.is_critical = false;
                state.last_update = Instant::now();
                return;
            }
        };

        let mut state = self.state.write();
        state.current_temp = current_temp;
        state.last_update = Instant::now();

        // Emergency throttling for critical temperature
        if current_temp >= CRITICAL_TEMP {
            state.intensity = MIN_INTENSITY;
            state.is_critical = true;
            warn!(
                "CRITICAL TEMPERATURE: {}°C - Emergency throttling!",
                current_temp / 1000
            );
            return;
        }

        state.is_critical = false;

        // PID control for smooth temperature regulation
        let temp_error = (current_temp - state.target_temp) as f32 / 1000.0; // Convert to Celsius
        let pid_output = self.pid.write().compute(temp_error);

        // Update intensity based on PID output
        // Positive error (too hot) -> reduce intensity
        // Negative error (too cool) -> increase intensity
        state.intensity = (state.intensity - pid_output * 0.1).clamp(MIN_INTENSITY, MAX_INTENSITY);

        debug!(
            "Thermal update: {}°C -> intensity {:.2}",
            current_temp / 1000,
            state.intensity
        );
    }

    /// Get current thermal state (thread-safe)
    pub fn state(&self) -> ThermalState {
        self.state.read().clone()
    }

    /// Get current mining intensity factor (0.1 - 1.0)
    pub fn intensity(&self) -> f32 {
        self.state.read().intensity
    }

    /// Check if system is in critical thermal state
    pub fn is_critical(&self) -> bool {
        self.state.read().is_critical
    }

    /// Set new target temperature (in Celsius)
    pub fn set_target_temp(&self, temp_celsius: f32) {
        let mut state = self.state.write();
        state.target_temp = (temp_celsius * 1000.0) as i32;
        info!("Thermal target updated to {}°C", temp_celsius);
    }

    /// Start thermal monitoring background task
    pub async fn start_monitoring(&self) -> tokio::task::JoinHandle<()> {
        let governor = Arc::new(self.clone());
        
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(500));
            
            loop {
                interval.tick().await;
                governor.update();
            }
        })
    }
}

// Enable cloning for Arc usage in async contexts
impl Clone for ThermalGovernor {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            pid: RwLock::new(PidController::new()), // Fresh PID controller for clone
            thermal_zones: self.thermal_zones.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_thermal_governor_creation() {
        let gov = ThermalGovernor::new();
        let state = gov.state();
        
        assert_eq!(state.target_temp, DEFAULT_TARGET_TEMP);
        assert_eq!(state.intensity, MAX_INTENSITY);
        assert!(!state.is_critical);
    }

    #[test]
    fn test_custom_target_temp() {
        let gov = ThermalGovernor::with_target_temp(60.0);
        let state = gov.state();
        
        assert_eq!(state.target_temp, 60_000);
    }

    #[test]
    fn test_intensity_bounds() {
        let gov = ThermalGovernor::new();
        let mut state = gov.state.write();
        
        state.intensity = -0.5;
        drop(state);
        
        // Intensity should be clamped to valid range
        let intensity = gov.intensity();
        assert!(intensity >= MIN_INTENSITY && intensity <= MAX_INTENSITY);
    }

    #[test]
    fn test_mock_thermal_reading() {
        // Create a temporary file to simulate thermal zone
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "55000").unwrap(); // 55°C
        
        let content = fs::read_to_string(temp_file.path()).unwrap();
        let temp = content.trim().parse::<i32>().unwrap();
        
        assert_eq!(temp, 55_000);
    }

    #[tokio::test]
    async fn test_thermal_monitoring_task() {
        let gov = ThermalGovernor::new();
        let handle = gov.start_monitoring().await;
        
        // Let it run briefly then cancel
        tokio::time::sleep(Duration::from_millis(100)).await;
        handle.abort();
        
        // Should complete without panic
    }

    #[test]
    fn test_pid_controller() {
        let mut pid = PidController::new();
        
        // Test error correction
        let output1 = pid.compute(5.0); // High positive error
        let output2 = pid.compute(2.0); // Lower error
        let output3 = pid.compute(-1.0); // Negative error
        
        // PID should respond to error changes
        assert!(output1 > 0.0); // Positive correction
        assert!(output3 < 0.0); // Negative correction
    }
}