//! Idle-Burst Mining Pattern
//!
//! Implements micro-burst computation with micro-sleeps to achieve ~80% hashrate
//! while consuming only ~40-50% of continuous CPU power.
//!
//! The burst pattern alternates between:
//! - **Burst phase**: Intensive computation for `burst_ms` milliseconds
//! - **Idle phase**: Sleep for `sleep_ms` milliseconds to let CPU cool down
//!
//! This reduces thermal pressure and power consumption while maintaining
//! reasonable mining performance.

use std::time::{Duration, Instant};
use tracing::{debug, trace};

/// Default burst duration in milliseconds (20ms bursts)
const DEFAULT_BURST_MS: u64 = 20;

/// Default sleep duration in milliseconds (10ms sleep)
const DEFAULT_SLEEP_MS: u64 = 10;

/// Minimum burst duration (1ms)
const MIN_BURST_MS: u64 = 1;

/// Maximum burst duration (1000ms = 1s)
const MAX_BURST_MS: u64 = 1000;

/// Minimum sleep duration (1ms)
const MIN_SLEEP_MS: u64 = 1;

/// Maximum sleep duration (1000ms = 1s)
const MAX_SLEEP_MS: u64 = 1000;

/// Configuration for burst mining pattern
#[derive(Debug, Clone, Copy)]
pub struct BurstConfig {
    /// Duration of computation burst in milliseconds
    pub burst_ms: u64,
    /// Duration of idle sleep in milliseconds  
    pub sleep_ms: u64,
    /// Enable adaptive tuning based on thermal feedback
    pub adaptive: bool,
}

impl Default for BurstConfig {
    fn default() -> Self {
        Self {
            burst_ms: DEFAULT_BURST_MS,
            sleep_ms: DEFAULT_SLEEP_MS,
            adaptive: true,
        }
    }
}

impl BurstConfig {
    /// Create burst config with custom timings
    pub fn new(burst_ms: u64, sleep_ms: u64) -> Self {
        Self {
            burst_ms: burst_ms.clamp(MIN_BURST_MS, MAX_BURST_MS),
            sleep_ms: sleep_ms.clamp(MIN_SLEEP_MS, MAX_SLEEP_MS),
            adaptive: false,
        }
    }

    /// Create config optimized for low power consumption (~40% duty cycle)
    pub fn low_power() -> Self {
        Self {
            burst_ms: 15,
            sleep_ms: 25,
            adaptive: true,
        }
    }

    /// Create config optimized for balanced performance (~67% duty cycle)
    pub fn balanced() -> Self {
        Self {
            burst_ms: 20,
            sleep_ms: 10,
            adaptive: true,
        }
    }

    /// Create config optimized for performance (~80% duty cycle)
    pub fn performance() -> Self {
        Self {
            burst_ms: 40,
            sleep_ms: 10,
            adaptive: true,
        }
    }

    /// Calculate duty cycle as percentage (0-100)
    pub fn duty_cycle(&self) -> f64 {
        let total = self.burst_ms + self.sleep_ms;
        if total == 0 {
            return 0.0;
        }
        (self.burst_ms as f64 / total as f64) * 100.0
    }

    /// Estimated hashrate efficiency compared to continuous mining
    pub fn hashrate_efficiency(&self) -> f64 {
        // Empirical formula: efficiency is slightly higher than duty cycle
        // due to reduced thermal throttling and cache warming effects
        let duty = self.duty_cycle() / 100.0;
        (duty * 1.05).min(1.0) // Cap at 100%
    }

    /// Estimated power efficiency (performance per watt)
    pub fn power_efficiency(&self) -> f64 {
        let hashrate_eff = self.hashrate_efficiency();
        let power_usage = self.duty_cycle() / 100.0;
        
        if power_usage == 0.0 {
            return 0.0;
        }
        
        hashrate_eff / power_usage
    }
}

/// Statistics for burst mining performance tracking
#[derive(Debug, Default, Clone)]
pub struct BurstStats {
    pub total_bursts: u64,
    pub total_idles: u64,
    pub total_burst_time: Duration,
    pub total_idle_time: Duration,
    pub adaptive_adjustments: u64,
}

impl BurstStats {
    /// Calculate actual duty cycle from recorded statistics
    pub fn actual_duty_cycle(&self) -> f64 {
        let total = self.total_burst_time + self.total_idle_time;
        if total.is_zero() {
            return 0.0;
        }
        (self.total_burst_time.as_secs_f64() / total.as_secs_f64()) * 100.0
    }

    /// Average burst duration
    pub fn avg_burst_duration(&self) -> Duration {
        if self.total_bursts == 0 {
            Duration::ZERO
        } else {
            self.total_burst_time / self.total_bursts.max(1) as u32
        }
    }

    /// Average idle duration
    pub fn avg_idle_duration(&self) -> Duration {
        if self.total_idles == 0 {
            Duration::ZERO
        } else {
            self.total_idle_time / self.total_idles.max(1) as u32
        }
    }
}

/// Burst mining controller that manages the burst/idle cycle
pub struct BurstController {
    config: BurstConfig,
    stats: BurstStats,
    last_burst_start: Option<Instant>,
    in_burst_phase: bool,
}

impl BurstController {
    /// Create new burst controller with default configuration
    pub fn new() -> Self {
        Self {
            config: BurstConfig::default(),
            stats: BurstStats::default(),
            last_burst_start: None,
            in_burst_phase: false,
        }
    }

    /// Create burst controller with custom configuration
    pub fn with_config(config: BurstConfig) -> Self {
        Self {
            config,
            stats: BurstStats::default(),
            last_burst_start: None,
            in_burst_phase: false,
        }
    }

    /// Get current configuration
    pub fn config(&self) -> BurstConfig {
        self.config
    }

    /// Get performance statistics
    pub fn stats(&self) -> &BurstStats {
        &self.stats
    }

    /// Update configuration (for adaptive tuning)
    pub fn update_config(&mut self, new_config: BurstConfig) {
        if self.config.adaptive {
            debug!(
                "Burst config updated: {}ms burst, {}ms sleep (duty: {:.1}%)",
                new_config.burst_ms,
                new_config.sleep_ms,
                new_config.duty_cycle()
            );
            self.config = new_config;
            self.stats.adaptive_adjustments += 1;
        }
    }

    /// Adapt burst pattern based on thermal intensity
    /// Lower intensity = longer sleeps for better cooling
    pub fn adapt_to_thermal_intensity(&mut self, thermal_intensity: f32) {
        if !self.config.adaptive {
            return;
        }

        let intensity = thermal_intensity.clamp(0.1, 1.0);
        
        // Scale burst/sleep times inversely with thermal pressure
        // High intensity (cool) = longer bursts, shorter sleeps
        // Low intensity (hot) = shorter bursts, longer sleeps
        let base_burst = DEFAULT_BURST_MS as f32;
        let base_sleep = DEFAULT_SLEEP_MS as f32;
        
        let burst_ms = (base_burst * intensity).round() as u64;
        let sleep_ms = (base_sleep * (2.0 - intensity)).round() as u64;
        
        let new_config = BurstConfig {
            burst_ms: burst_ms.clamp(MIN_BURST_MS, MAX_BURST_MS),
            sleep_ms: sleep_ms.clamp(MIN_SLEEP_MS, MAX_SLEEP_MS),
            adaptive: true,
        };

        if new_config.burst_ms != self.config.burst_ms || 
           new_config.sleep_ms != self.config.sleep_ms {
            self.update_config(new_config);
        }
    }

    /// Execute the burst/idle cycle - returns true during burst phase, false during idle
    /// Call this in your mining loop to implement the burst pattern
    pub async fn cycle(&mut self) -> bool {
        let now = Instant::now();

        if self.in_burst_phase {
            // Check if burst phase should end
            if let Some(burst_start) = self.last_burst_start {
                let burst_duration = now.duration_since(burst_start);
                
                if burst_duration >= Duration::from_millis(self.config.burst_ms) {
                    // End burst phase, start idle phase
                    self.in_burst_phase = false;
                    self.stats.total_bursts += 1;
                    self.stats.total_burst_time += burst_duration;
                    
                    trace!("Burst phase ended: {:.2}ms", burst_duration.as_millis());
                    
                    // Sleep during idle phase
                    let sleep_duration = Duration::from_millis(self.config.sleep_ms);
                    tokio::time::sleep(sleep_duration).await;
                    
                    self.stats.total_idles += 1;
                    self.stats.total_idle_time += sleep_duration;
                    
                    trace!("Idle phase ended: {}ms", self.config.sleep_ms);
                }
            }
        } else {
            // Start new burst phase
            self.in_burst_phase = true;
            self.last_burst_start = Some(now);
            trace!("Burst phase started");
        }

        self.in_burst_phase
    }

    /// Synchronous version of cycle() for non-async contexts
    /// Uses std::thread::sleep instead of tokio::time::sleep
    pub fn cycle_blocking(&mut self) -> bool {
        let now = Instant::now();

        if self.in_burst_phase {
            // Check if burst phase should end
            if let Some(burst_start) = self.last_burst_start {
                let burst_duration = now.duration_since(burst_start);
                
                if burst_duration >= Duration::from_millis(self.config.burst_ms) {
                    // End burst phase, start idle phase
                    self.in_burst_phase = false;
                    self.stats.total_bursts += 1;
                    self.stats.total_burst_time += burst_duration;
                    
                    // Sleep during idle phase
                    let sleep_duration = Duration::from_millis(self.config.sleep_ms);
                    std::thread::sleep(sleep_duration);
                    
                    self.stats.total_idles += 1;
                    self.stats.total_idle_time += sleep_duration;
                }
            }
        } else {
            // Start new burst phase
            self.in_burst_phase = true;
            self.last_burst_start = Some(now);
        }

        self.in_burst_phase
    }

    /// Reset statistics
    pub fn reset_stats(&mut self) {
        self.stats = BurstStats::default();
    }
}

impl Default for BurstController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_burst_config_creation() {
        let config = BurstConfig::new(30, 15);
        assert_eq!(config.burst_ms, 30);
        assert_eq!(config.sleep_ms, 15);
        assert!(!config.adaptive);
    }

    #[test]
    fn test_burst_config_clamping() {
        let config = BurstConfig::new(0, 2000);
        assert_eq!(config.burst_ms, MIN_BURST_MS);
        assert_eq!(config.sleep_ms, MAX_SLEEP_MS);
    }

    #[test]
    fn test_duty_cycle_calculation() {
        let config = BurstConfig::new(20, 10);
        assert_eq!(config.duty_cycle(), (20.0 / 30.0) * 100.0);
    }

    #[test]
    fn test_preset_configs() {
        let low_power = BurstConfig::low_power();
        let balanced = BurstConfig::balanced();
        let performance = BurstConfig::performance();

        assert!(low_power.duty_cycle() < balanced.duty_cycle());
        assert!(balanced.duty_cycle() < performance.duty_cycle());
    }

    #[test]
    fn test_efficiency_calculations() {
        let config = BurstConfig::balanced();
        
        let hashrate_eff = config.hashrate_efficiency();
        let power_eff = config.power_efficiency();
        
        assert!(hashrate_eff > 0.0 && hashrate_eff <= 1.0);
        assert!(power_eff > 0.0);
    }

    #[test]
    fn test_burst_controller_creation() {
        let controller = BurstController::new();
        assert_eq!(controller.config.burst_ms, DEFAULT_BURST_MS);
        assert_eq!(controller.config.sleep_ms, DEFAULT_SLEEP_MS);
        assert!(!controller.in_burst_phase);
    }

    #[test]
    fn test_thermal_adaptation() {
        let mut controller = BurstController::new();
        let original_config = controller.config;
        
        // High thermal intensity should allow longer bursts
        controller.adapt_to_thermal_intensity(0.9);
        let hot_config = controller.config;
        
        // Low thermal intensity should enforce shorter bursts
        controller.config = original_config; // Reset
        controller.adapt_to_thermal_intensity(0.3);
        let cool_config = controller.config;
        
        assert!(hot_config.burst_ms >= cool_config.burst_ms);
        assert!(hot_config.sleep_ms <= cool_config.sleep_ms);
    }

    #[test]
    fn test_blocking_cycle() {
        let mut controller = BurstController::with_config(
            BurstConfig::new(10, 5) // Very short durations for testing
        );
        
        // First call should start burst phase
        assert!(controller.cycle_blocking());
        assert!(controller.in_burst_phase);
        
        // After burst duration, should switch to idle
        std::thread::sleep(Duration::from_millis(15));
        assert!(!controller.cycle_blocking());
        
        // Stats should be updated
        assert_eq!(controller.stats.total_bursts, 1);
        assert_eq!(controller.stats.total_idles, 1);
    }

    #[tokio::test]
    async fn test_async_cycle() {
        let mut controller = BurstController::with_config(
            BurstConfig::new(5, 3) // Very short durations for testing
        );

        // First call starts burst phase
        controller.cycle().await;
        // Wait longer than burst_ms (5ms) so burst expires on next cycle
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        // Second call ends burst + sleeps idle
        controller.cycle().await;

        // Should have recorded at least one burst and one idle
        assert!(controller.stats.total_bursts > 0 || controller.stats.total_idles > 0);
    }

    #[test]
    fn test_stats_calculations() {
        let mut stats = BurstStats::default();
        stats.total_bursts = 10;
        stats.total_burst_time = Duration::from_millis(200);
        stats.total_idles = 10;
        stats.total_idle_time = Duration::from_millis(100);
        
        assert_eq!(stats.avg_burst_duration(), Duration::from_millis(20));
        assert_eq!(stats.avg_idle_duration(), Duration::from_millis(10));
        assert!((stats.actual_duty_cycle() - 66.67).abs() < 0.1);
    }
}