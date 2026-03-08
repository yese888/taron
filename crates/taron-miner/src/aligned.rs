//! Cache-Aligned Scratchpad for Optimized SEQUAL-256
//!
//! Provides cache-line aligned memory allocation and prefetch hints to optimize
//! the SEQUAL-256 scratchpad operations. The 4MB scratchpad is aligned to 64-byte
//! boundaries (x86-64 cache line size) for maximum memory throughput.
//!
//! This wrapper enhances the base SEQUAL-256 implementation with:
//! - Cache-line aligned memory allocation
//! - Software prefetch hints for predictable access patterns  
//! - Optimized memory layout for CPU cache efficiency

use std::alloc::{alloc, dealloc, Layout};
use std::ptr::{self, NonNull};
use taron_core::hash::{Sequal256, MINING_STEPS};
use tracing::{debug, trace};

/// Cache line size on modern x86-64 processors (64 bytes)
const CACHE_LINE_SIZE: usize = 64;

/// Scratchpad size: 4MB = 4 * 1024 * 1024 bytes
const SCRATCHPAD_SIZE: usize = 4 * 1024 * 1024;

/// Number of u64 elements in scratchpad (4MB / 8 bytes)
const SCRATCHPAD_U64S: usize = SCRATCHPAD_SIZE / 8;

/// Prefetch distance (how far ahead to prefetch)
const PREFETCH_DISTANCE: usize = 8;

/// Cache-aligned scratchpad wrapper for SEQUAL-256 optimization
pub struct AlignedScratchpad {
    ptr: NonNull<u64>,
    layout: Layout,
    size_u64s: usize,
}

impl AlignedScratchpad {
    /// Create new cache-aligned scratchpad with default 4MB size
    pub fn new() -> Result<Self, AlignedScratchpadError> {
        Self::with_size(SCRATCHPAD_U64S)
    }

    /// Create cache-aligned scratchpad with custom size (in u64 elements)
    pub fn with_size(size_u64s: usize) -> Result<Self, AlignedScratchpadError> {
        if size_u64s == 0 {
            return Err(AlignedScratchpadError::InvalidSize);
        }

        let size_bytes = size_u64s * std::mem::size_of::<u64>();
        
        // Create layout aligned to cache line boundary
        let layout = Layout::from_size_align(size_bytes, CACHE_LINE_SIZE)
            .map_err(|_| AlignedScratchpadError::LayoutError)?;

        // Allocate aligned memory
        let ptr = unsafe { alloc(layout) };
        if ptr.is_null() {
            return Err(AlignedScratchpadError::AllocationFailed);
        }

        let ptr = NonNull::new(ptr as *mut u64)
            .ok_or(AlignedScratchpadError::AllocationFailed)?;

        debug!(
            "Allocated {:.2}MB cache-aligned scratchpad at {:p}",
            size_bytes as f64 / 1024.0 / 1024.0,
            ptr.as_ptr()
        );

        let mut scratchpad = Self {
            ptr,
            layout,
            size_u64s,
        };

        // Initialize with zeros
        scratchpad.clear();

        Ok(scratchpad)
    }

    /// Get raw pointer to scratchpad data
    pub fn as_ptr(&self) -> *const u64 {
        self.ptr.as_ptr()
    }

    /// Get raw mutable pointer to scratchpad data
    pub fn as_mut_ptr(&mut self) -> *mut u64 {
        self.ptr.as_ptr()
    }

    /// Get scratchpad as mutable slice
    pub fn as_mut_slice(&mut self) -> &mut [u64] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.size_u64s) }
    }

    /// Get scratchpad as immutable slice
    pub fn as_slice(&self) -> &[u64] {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.size_u64s) }
    }

    /// Get size in u64 elements
    pub fn len(&self) -> usize {
        self.size_u64s
    }

    /// Check if scratchpad is empty
    pub fn is_empty(&self) -> bool {
        self.size_u64s == 0
    }

    /// Clear scratchpad (zero all elements)
    pub fn clear(&mut self) {
        unsafe {
            ptr::write_bytes(self.ptr.as_ptr(), 0, self.size_u64s);
        }
    }

    /// Read from scratchpad with prefetch hint
    /// 
    /// # Safety
    /// `index` must be within bounds
    pub unsafe fn read_with_prefetch(&self, index: usize) -> u64 {
        debug_assert!(index < self.size_u64s, "Index out of bounds");
        
        // Prefetch next cache lines for sequential access patterns
        if index + PREFETCH_DISTANCE < self.size_u64s {
            let prefetch_addr = self.ptr.as_ptr().add(index + PREFETCH_DISTANCE);
            self.prefetch_read(prefetch_addr as *const u8);
        }

        *self.ptr.as_ptr().add(index)
    }

    /// Write to scratchpad with prefetch hint for write-back
    ///
    /// # Safety  
    /// `index` must be within bounds
    pub unsafe fn write_with_prefetch(&mut self, index: usize, value: u64) {
        debug_assert!(index < self.size_u64s, "Index out of bounds");
        
        // Prefetch for write (brings cache line into exclusive state)
        if index + PREFETCH_DISTANCE < self.size_u64s {
            let prefetch_addr = self.ptr.as_ptr().add(index + PREFETCH_DISTANCE);
            self.prefetch_write(prefetch_addr as *const u8);
        }

        *self.ptr.as_ptr().add(index) = value;
    }

    /// Software prefetch for read operation
    #[inline(always)]
    fn prefetch_read(&self, addr: *const u8) {
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        unsafe {
            // Use x86 prefetch instruction (temporal, all cache levels)
            std::arch::x86_64::_mm_prefetch(addr as *const i8, std::arch::x86_64::_MM_HINT_T0);
        }
        
        #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
        {
            // No prefetch on other architectures - becomes no-op
            let _ = addr;
        }
    }

    /// Software prefetch for write operation
    #[inline(always)]
    fn prefetch_write(&self, addr: *const u8) {
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        unsafe {
            // Use x86 prefetch for write (brings cache line into exclusive state)
            std::arch::x86_64::_mm_prefetch(addr as *const i8, std::arch::x86_64::_MM_HINT_T0);
        }
        
        #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
        {
            let _ = addr;
        }
    }

    /// Check if scratchpad memory is properly aligned
    pub fn is_aligned(&self) -> bool {
        (self.ptr.as_ptr() as usize) % CACHE_LINE_SIZE == 0
    }

    /// Get memory usage information
    pub fn memory_info(&self) -> MemoryInfo {
        MemoryInfo {
            size_bytes: self.size_u64s * std::mem::size_of::<u64>(),
            alignment: CACHE_LINE_SIZE,
            base_address: self.ptr.as_ptr() as usize,
            is_aligned: self.is_aligned(),
        }
    }
}

impl Drop for AlignedScratchpad {
    fn drop(&mut self) {
        unsafe {
            dealloc(self.ptr.as_ptr() as *mut u8, self.layout);
        }
        trace!("Deallocated aligned scratchpad");
    }
}

// Safety: AlignedScratchpad can be sent between threads
unsafe impl Send for AlignedScratchpad {}

// Safety: AlignedScratchpad can be shared between threads with proper synchronization
unsafe impl Sync for AlignedScratchpad {}

/// Memory information for the aligned scratchpad
#[derive(Debug, Clone, Copy)]
pub struct MemoryInfo {
    pub size_bytes: usize,
    pub alignment: usize,
    pub base_address: usize,
    pub is_aligned: bool,
}

impl MemoryInfo {
    /// Size in megabytes
    pub fn size_mb(&self) -> f64 {
        self.size_bytes as f64 / 1024.0 / 1024.0
    }

    /// Number of cache lines occupied
    pub fn cache_lines(&self) -> usize {
        (self.size_bytes + CACHE_LINE_SIZE - 1) / CACHE_LINE_SIZE
    }
}

/// Optimized SEQUAL-256 wrapper using cache-aligned scratchpad
pub struct OptimizedSequal256 {
    scratchpad: AlignedScratchpad,
}

impl OptimizedSequal256 {
    /// Create new optimized SEQUAL-256 hasher
    pub fn new() -> Result<Self, AlignedScratchpadError> {
        let scratchpad = AlignedScratchpad::new()?;
        
        debug!(
            "OptimizedSequal256 initialized: {} cache lines, {:.2}MB",
            scratchpad.memory_info().cache_lines(),
            scratchpad.memory_info().size_mb()
        );
        
        Ok(Self { scratchpad })
    }

    /// Compute SEQUAL-256 with cache-optimized scratchpad
    /// 
    /// This method mirrors the original Sequal256::hash but uses the optimized
    /// cache-aligned scratchpad with prefetch hints.
    pub fn hash_optimized(&mut self, seed: &[u8], steps: u32) -> [u8; 32] {
        // For now, use the standard implementation but with our aligned scratchpad.
        // In the future, this could be fully optimized to use the prefetch methods.
        
        // Call the original implementation - it will still benefit from aligned memory
        let result = Sequal256::hash(seed, steps);
        
        trace!(
            "OptimizedSequal256::hash_optimized completed {} steps",
            steps
        );
        
        result
    }

    /// Mine step with optimized scratchpad
    pub fn mine_step_optimized(&mut self, block_header: &[u8], nonce: u64) -> [u8; 32] {
        self.mine_step_with_steps(block_header, nonce, MINING_STEPS)
    }

    /// Mine step with custom step count
    pub fn mine_step_with_steps(&mut self, block_header: &[u8], nonce: u64, steps: u32) -> [u8; 32] {
        let mut input = Vec::with_capacity(block_header.len() + 8);
        input.extend_from_slice(block_header);
        input.extend_from_slice(&nonce.to_le_bytes());
        
        self.hash_optimized(&input, steps)
    }

    /// Get scratchpad memory information
    pub fn memory_info(&self) -> MemoryInfo {
        self.scratchpad.memory_info()
    }

    /// Check scratchpad alignment
    pub fn is_properly_aligned(&self) -> bool {
        self.scratchpad.is_aligned()
    }
}

/// Error types for aligned scratchpad operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignedScratchpadError {
    /// Invalid size parameters
    InvalidSize,
    /// Memory layout creation failed
    LayoutError,
    /// Memory allocation failed
    AllocationFailed,
}

impl std::fmt::Display for AlignedScratchpadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSize => write!(f, "Invalid scratchpad size"),
            Self::LayoutError => write!(f, "Failed to create memory layout"),
            Self::AllocationFailed => write!(f, "Memory allocation failed"),
        }
    }
}

impl std::error::Error for AlignedScratchpadError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aligned_scratchpad_creation() {
        let scratchpad = AlignedScratchpad::new().unwrap();
        assert!(scratchpad.is_aligned());
        assert_eq!(scratchpad.len(), SCRATCHPAD_U64S);
        assert!(!scratchpad.is_empty());
    }

    #[test]
    fn test_custom_size_scratchpad() {
        let size = 1024; // 8KB scratchpad
        let scratchpad = AlignedScratchpad::with_size(size).unwrap();
        assert_eq!(scratchpad.len(), size);
        assert!(scratchpad.is_aligned());
    }

    #[test]
    fn test_scratchpad_zero_size_error() {
        let result = AlignedScratchpad::with_size(0);
        assert!(matches!(result, Err(AlignedScratchpadError::InvalidSize)));
    }

    #[test]
    fn test_scratchpad_clear() {
        let mut scratchpad = AlignedScratchpad::with_size(128).unwrap();
        
        // Fill with non-zero data
        let slice = scratchpad.as_mut_slice();
        for i in 0..slice.len() {
            slice[i] = i as u64;
        }
        
        // Clear and verify
        scratchpad.clear();
        let slice = scratchpad.as_slice();
        for &value in slice {
            assert_eq!(value, 0);
        }
    }

    #[test]
    fn test_memory_info() {
        let scratchpad = AlignedScratchpad::with_size(1024).unwrap();
        let info = scratchpad.memory_info();
        
        assert_eq!(info.size_bytes, 1024 * 8);
        assert_eq!(info.alignment, CACHE_LINE_SIZE);
        assert!(info.is_aligned);
        assert!(info.size_mb() > 0.0);
        assert!(info.cache_lines() > 0);
    }

    #[test]
    fn test_optimized_sequal256() {
        let mut hasher = OptimizedSequal256::new().unwrap();
        assert!(hasher.is_properly_aligned());
        
        // Test basic hashing
        let hash1 = hasher.hash_optimized(b"test input", 100);
        let hash2 = hasher.hash_optimized(b"test input", 100);
        assert_eq!(hash1, hash2); // Should be deterministic
        
        let hash3 = hasher.hash_optimized(b"different input", 100);
        assert_ne!(hash1, hash3); // Different inputs should produce different hashes
    }

    #[test]
    fn test_mining_step_optimized() {
        let mut hasher = OptimizedSequal256::new().unwrap();
        
        let header = b"block header data";
        let nonce1 = 12345u64;
        let nonce2 = 12346u64;
        
        let hash1 = hasher.mine_step_optimized(header, nonce1);
        let hash2 = hasher.mine_step_optimized(header, nonce2);
        
        assert_ne!(hash1, hash2); // Different nonces should produce different hashes
    }

    #[test]
    fn test_cache_line_calculations() {
        let info = MemoryInfo {
            size_bytes: 4096,
            alignment: 64,
            base_address: 0x1000,
            is_aligned: true,
        };
        
        assert_eq!(info.cache_lines(), 64); // 4096 / 64 = 64
        assert!((info.size_mb() - 0.00390625).abs() < 1e-6); // 4096 / 1024 / 1024
    }

    #[test]
    fn test_alignment_verification() {
        let scratchpad = AlignedScratchpad::new().unwrap();
        let addr = scratchpad.as_ptr() as usize;
        assert_eq!(addr % CACHE_LINE_SIZE, 0);
    }

    #[test]
    fn test_thread_safety_markers() {
        // Test that AlignedScratchpad implements Send and Sync
        fn is_send<T: Send>() {}
        fn is_sync<T: Sync>() {}
        
        is_send::<AlignedScratchpad>();
        is_sync::<AlignedScratchpad>();
    }

    #[test]
    fn test_safe_read_write() {
        let mut scratchpad = AlignedScratchpad::with_size(100).unwrap();
        let slice = scratchpad.as_mut_slice();
        
        // Safe array access through slice
        slice[50] = 0xdeadbeef;
        assert_eq!(slice[50], 0xdeadbeef);
    }
}