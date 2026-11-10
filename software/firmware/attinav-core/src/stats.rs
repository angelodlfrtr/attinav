//! Running statistics for shot averaging.

use crate::math::Vec3;

/// Welford running mean and variance of a stream of 3-vectors.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Accumulator3 {
    n: u32,
    mean: Vec3,
    m2: Vec3,
}

impl Accumulator3 {
    /// Empty accumulator.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            n: 0,
            mean: Vec3::ZERO,
            m2: Vec3::ZERO,
        }
    }

    /// Adds one sample.
    pub fn push(&mut self, v: Vec3) {
        self.n += 1;
        let n = f64::from(self.n);
        let delta = v - self.mean;
        self.mean = self.mean + delta * (1.0 / n);
        let delta2 = v - self.mean;
        self.m2 = Vec3::new(
            self.m2.x + delta.x * delta2.x,
            self.m2.y + delta.y * delta2.y,
            self.m2.z + delta.z * delta2.z,
        );
    }

    /// Number of samples.
    #[must_use]
    pub const fn count(&self) -> u32 {
        self.n
    }

    /// Mean (zero when empty).
    #[must_use]
    pub const fn mean(&self) -> Vec3 {
        self.mean
    }

    /// Per-axis sample standard deviation (zero with fewer than two samples).
    #[must_use]
    pub fn std_dev(&self) -> Vec3 {
        if self.n < 2 {
            return Vec3::ZERO;
        }
        let d = f64::from(self.n - 1);
        Vec3::new(
            libm::sqrt(self.m2.x / d),
            libm::sqrt(self.m2.y / d),
            libm::sqrt(self.m2.z / d),
        )
    }

    /// Standard error of the mean (per axis).
    #[must_use]
    pub fn std_err(&self) -> Vec3 {
        if self.n == 0 {
            return Vec3::ZERO;
        }
        self.std_dev() * (1.0 / libm::sqrt(f64::from(self.n)))
    }

    /// Frozen summary.
    #[must_use]
    pub fn summary(&self) -> SampleStats {
        SampleStats {
            count: self.n,
            mean: self.mean,
            std_dev: self.std_dev(),
        }
    }
}

/// Summary of one sensor over one shot.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SampleStats {
    /// Number of samples averaged.
    pub count: u32,
    /// Mean, in raw sensor counts.
    pub mean: Vec3,
    /// Sample standard deviation per axis, in raw sensor counts.
    pub std_dev: Vec3,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mean_and_variance() {
        let mut a = Accumulator3::new();
        for i in 0..5 {
            let v = f64::from(i);
            a.push(Vec3::new(v, 2.0 * v, -v));
        }
        assert_eq!(a.count(), 5);
        assert!((a.mean().x - 2.0).abs() < 1e-12);
        assert!((a.mean().y - 4.0).abs() < 1e-12);
        // Sample variance of 0..4 is 2.5.
        assert!((a.std_dev().x - 2.5f64.sqrt()).abs() < 1e-12);
        assert!((a.std_dev().y - (4.0 * 2.5f64).sqrt()).abs() < 1e-12);
        assert!((a.std_err().x - (2.5f64 / 5.0).sqrt()).abs() < 1e-12);
    }

    #[test]
    fn empty_is_safe() {
        let a = Accumulator3::new();
        assert_eq!(a.std_dev(), Vec3::ZERO);
        assert_eq!(a.std_err(), Vec3::ZERO);
        assert_eq!(a.summary().count, 0);
    }
}
