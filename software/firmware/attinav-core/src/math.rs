//! Minimal fixed-size linear algebra in `f64`.
//!
//! `f64` everywhere: the Cortex-M33 only has a single-precision FPU, but the per-shot work is a
//! few hundred operations and the solver runs offline, so the soft-float cost is irrelevant next to
//! the conditioning problems `f32` would create in the normal equations.

use core::ops::{Add, Index, IndexMut, Mul, Neg, Sub};

/// A 3-vector.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Vec3 {
    /// X component.
    pub x: f64,
    /// Y component.
    pub y: f64,
    /// Z component.
    pub z: f64,
}

impl Vec3 {
    /// The zero vector.
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);
    /// Unit X.
    pub const X: Self = Self::new(1.0, 0.0, 0.0);
    /// Unit Y.
    pub const Y: Self = Self::new(0.0, 1.0, 0.0);
    /// Unit Z.
    pub const Z: Self = Self::new(0.0, 0.0, 1.0);

    /// Builds a vector from components.
    #[must_use]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    /// Builds a vector from an array.
    #[must_use]
    pub const fn from_array(a: [f64; 3]) -> Self {
        Self::new(a[0], a[1], a[2])
    }

    /// Components as an array.
    #[must_use]
    pub const fn to_array(self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }

    /// Components as `f32` (wire format).
    #[must_use]
    pub const fn to_f32_array(self) -> [f32; 3] {
        [self.x as f32, self.y as f32, self.z as f32]
    }

    /// Dot product.
    #[must_use]
    pub fn dot(self, o: Self) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    /// Cross product `self × o`.
    #[must_use]
    pub fn cross(self, o: Self) -> Self {
        Self::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    /// Squared Euclidean norm.
    #[must_use]
    pub fn norm_sq(self) -> f64 {
        self.dot(self)
    }

    /// Euclidean norm.
    #[must_use]
    pub fn norm(self) -> f64 {
        libm::sqrt(self.norm_sq())
    }

    /// Unit vector in the same direction, or `None` for a (near-)zero vector.
    #[must_use]
    pub fn normalized(self) -> Option<Self> {
        let n = self.norm();
        (n > 1e-300).then(|| self * (1.0 / n))
    }

    /// Component-wise scaling.
    #[must_use]
    pub fn scale(self, s: f64) -> Self {
        self * s
    }

    /// Angle between two vectors, in radians, robust near 0 and π.
    #[must_use]
    pub fn angle_to(self, o: Self) -> f64 {
        libm::atan2(self.cross(o).norm(), self.dot(o))
    }

    /// Largest absolute component.
    #[must_use]
    pub fn max_abs(self) -> f64 {
        libm::fabs(self.x).max(libm::fabs(self.y)).max(libm::fabs(self.z))
    }
}

impl Add for Vec3 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl Sub for Vec3 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl Neg for Vec3 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

impl Mul<f64> for Vec3 {
    type Output = Self;
    fn mul(self, s: f64) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s)
    }
}

impl Index<usize> for Vec3 {
    type Output = f64;
    fn index(&self, i: usize) -> &f64 {
        match i {
            0 => &self.x,
            1 => &self.y,
            2 => &self.z,
            _ => panic!("Vec3 index out of range"),
        }
    }
}

impl IndexMut<usize> for Vec3 {
    fn index_mut(&mut self, i: usize) -> &mut f64 {
        match i {
            0 => &mut self.x,
            1 => &mut self.y,
            2 => &mut self.z,
            _ => panic!("Vec3 index out of range"),
        }
    }
}

/// A 3×3 matrix, row-major.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Mat3 {
    /// Rows.
    pub m: [[f64; 3]; 3],
}

impl Default for Mat3 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Mat3 {
    /// Identity matrix.
    pub const IDENTITY: Self = Self {
        m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    };

    /// Builds a matrix from rows.
    #[must_use]
    pub const fn from_rows(r0: [f64; 3], r1: [f64; 3], r2: [f64; 3]) -> Self {
        Self { m: [r0, r1, r2] }
    }

    /// Builds a matrix from a row-major flat array.
    #[must_use]
    pub const fn from_flat(a: [f64; 9]) -> Self {
        Self::from_rows([a[0], a[1], a[2]], [a[3], a[4], a[5]], [a[6], a[7], a[8]])
    }

    /// Row-major flat array.
    #[must_use]
    pub const fn to_flat(self) -> [f64; 9] {
        let m = self.m;
        [
            m[0][0], m[0][1], m[0][2], m[1][0], m[1][1], m[1][2], m[2][0], m[2][1], m[2][2],
        ]
    }

    /// Diagonal matrix.
    #[must_use]
    pub const fn diag(d: Vec3) -> Self {
        Self::from_rows([d.x, 0.0, 0.0], [0.0, d.y, 0.0], [0.0, 0.0, d.z])
    }

    /// Uniform scaling.
    #[must_use]
    pub const fn scaling(s: f64) -> Self {
        Self::diag(Vec3::new(s, s, s))
    }

    /// Row `i`.
    #[must_use]
    pub const fn row(&self, i: usize) -> Vec3 {
        Vec3::from_array(self.m[i])
    }

    /// Column `j`.
    #[must_use]
    pub const fn col(&self, j: usize) -> Vec3 {
        Vec3::new(self.m[0][j], self.m[1][j], self.m[2][j])
    }

    /// Transpose.
    #[must_use]
    pub const fn transpose(&self) -> Self {
        let m = self.m;
        Self::from_rows(
            [m[0][0], m[1][0], m[2][0]],
            [m[0][1], m[1][1], m[2][1]],
            [m[0][2], m[1][2], m[2][2]],
        )
    }

    /// Determinant.
    #[must_use]
    pub fn det(&self) -> f64 {
        self.row(0).dot(self.row(1).cross(self.row(2)))
    }

    /// Inverse, or `None` when singular.
    #[must_use]
    pub fn inverse(&self) -> Option<Self> {
        let det = self.det();
        if libm::fabs(det) < 1e-300 {
            return None;
        }
        let r0 = self.row(0);
        let r1 = self.row(1);
        let r2 = self.row(2);
        // Rows of the adjugate are the cross products of the original rows (transposed cofactors).
        let c0 = r1.cross(r2);
        let c1 = r2.cross(r0);
        let c2 = r0.cross(r1);
        let inv = 1.0 / det;
        Some(Self::from_rows(
            [c0.x * inv, c1.x * inv, c2.x * inv],
            [c0.y * inv, c1.y * inv, c2.y * inv],
            [c0.z * inv, c1.z * inv, c2.z * inv],
        ))
    }

    /// Rotation about Z by `angle` radians (right-handed).
    #[must_use]
    pub fn rot_z(angle: f64) -> Self {
        let (s, c) = libm::sincos(angle);
        Self::from_rows([c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0])
    }

    /// Rotation about Y by `angle` radians (right-handed).
    #[must_use]
    pub fn rot_y(angle: f64) -> Self {
        let (s, c) = libm::sincos(angle);
        Self::from_rows([c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c])
    }

    /// Rotation about X by `angle` radians (right-handed).
    #[must_use]
    pub fn rot_x(angle: f64) -> Self {
        let (s, c) = libm::sincos(angle);
        Self::from_rows([1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c])
    }

    /// Frobenius norm of `self - o`, a cheap matrix distance for tests and convergence checks.
    #[must_use]
    pub fn distance(&self, o: &Self) -> f64 {
        let mut acc = 0.0;
        for i in 0..3 {
            for j in 0..3 {
                let d = self.m[i][j] - o.m[i][j];
                acc += d * d;
            }
        }
        libm::sqrt(acc)
    }
}

impl Mul<Vec3> for Mat3 {
    type Output = Vec3;
    fn mul(self, v: Vec3) -> Vec3 {
        Vec3::new(self.row(0).dot(v), self.row(1).dot(v), self.row(2).dot(v))
    }
}

impl Mul<Self> for Mat3 {
    type Output = Self;
    fn mul(self, o: Self) -> Self {
        let mut r = [[0.0; 3]; 3];
        for (i, row) in r.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = self.row(i).dot(o.col(j));
            }
        }
        Self { m: r }
    }
}

impl Mul<f64> for Mat3 {
    type Output = Self;
    fn mul(self, s: f64) -> Self {
        let mut r = self;
        for row in &mut r.m {
            for cell in row.iter_mut() {
                *cell *= s;
            }
        }
        r
    }
}

impl Add for Mat3 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        let mut r = self;
        for (i, row) in r.m.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell += o.m[i][j];
            }
        }
        r
    }
}

/// Degrees to radians.
#[must_use]
pub fn deg_to_rad(d: f64) -> f64 {
    d * (core::f64::consts::PI / 180.0)
}

/// Radians to degrees.
#[must_use]
pub fn rad_to_deg(r: f64) -> f64 {
    r * (180.0 / core::f64::consts::PI)
}

/// Wraps an angle in degrees into `[0, 360)`.
#[must_use]
pub fn wrap_deg_360(d: f64) -> f64 {
    let w = d % 360.0;
    let w = if w < 0.0 { w + 360.0 } else { w };
    // A tiny negative remainder plus 360 rounds to exactly 360.0.
    if w >= 360.0 { 0.0 } else { w }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-12
    }

    #[test]
    fn cross_and_dot() {
        assert_eq!(Vec3::X.cross(Vec3::Y), Vec3::Z);
        assert_eq!(Vec3::Y.cross(Vec3::Z), Vec3::X);
        assert!(approx(Vec3::new(1.0, 2.0, 3.0).dot(Vec3::new(4.0, 5.0, 6.0)), 32.0));
    }

    #[test]
    fn inverse_roundtrip() {
        let a = Mat3::from_rows([2.0, 0.3, -0.1], [0.1, 1.5, 0.2], [-0.4, 0.05, 0.9]);
        let inv = a.inverse().unwrap();
        assert!((a * inv).distance(&Mat3::IDENTITY) < 1e-12);
        assert!((inv * a).distance(&Mat3::IDENTITY) < 1e-12);
        assert!(
            Mat3::from_rows([1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [0.0, 0.0, 1.0])
                .inverse()
                .is_none()
        );
    }

    #[test]
    fn rotations_are_right_handed() {
        let r = Mat3::rot_z(core::f64::consts::FRAC_PI_2);
        let v = r * Vec3::X;
        assert!(approx(v.x, 0.0) && approx(v.y, 1.0));
        let r = Mat3::rot_x(core::f64::consts::FRAC_PI_2);
        let v = r * Vec3::Y;
        assert!(approx(v.y, 0.0) && approx(v.z, 1.0));
        let r = Mat3::rot_y(core::f64::consts::FRAC_PI_2);
        let v = r * Vec3::Z;
        assert!(approx(v.z, 0.0) && approx(v.x, 1.0));
    }

    #[test]
    fn angle_and_wrap() {
        assert!(approx(Vec3::X.angle_to(Vec3::Y), core::f64::consts::FRAC_PI_2));
        assert!(approx(wrap_deg_360(-10.0), 350.0));
        assert!(approx(wrap_deg_360(370.0), 10.0));
        assert_eq!(wrap_deg_360(-1e-20), 0.0);
        assert_eq!(wrap_deg_360(360.0), 0.0);
    }
}
