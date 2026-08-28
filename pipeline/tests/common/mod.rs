//! Shared test-only helpers: a seeded LCG (no `rand` dependency) used by
//! every synthetic-graph builder in this test suite.

/// A minimal LCG (numerical-recipes constants) so test graphs are
/// deterministic without pulling in a `rand` dependency.
pub struct Lcg(u64);

impl Lcg {
    pub fn new(seed: u64) -> Self {
        Lcg(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    /// Uniform in `[0, 1)`.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn next_range_u32(&mut self, lo: u32, hi: u32) -> u32 {
        lo + (self.next_f64() * (hi - lo) as f64) as u32
    }

    pub fn next_range_f32(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.next_f64() as f32 * (hi - lo)
    }
}
