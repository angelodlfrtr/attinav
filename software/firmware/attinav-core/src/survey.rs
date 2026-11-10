//! Survey geometry: azimuth, inclination and roll from a gravity and a magnetic field vector.
//!
//! # Frame conventions (used throughout the crate)
//!
//! * **Body frame** — right-handed, `X` forward along the laser axis, `Y` to the right, `Z` down.
//!   Both accelerometers and the three magnetometer triads are brought into this frame by their
//!   calibration; the body frame *is* the accelerometer-1 frame (its calibration matrix is upper
//!   triangular, see [`crate::solver`]), so the laser–accelerometer alignment is mechanical.
//! * **World frame** — NED: `N` magnetic north, `E` east, `D` down.
//! * `g` is the **direction of gravity** in the body frame (points down). An accelerometer at rest
//!   measures the reaction force, which points up: `g = -a / |a|`.
//! * `m` is the magnetic field in the body frame. In the northern hemisphere it points north *and*
//!   down (positive dip).
//!
//! Azimuth is the bearing of the laser axis clockwise from magnetic north in `[0°, 360°)`,
//! inclination is positive above the horizon in `[-90°, 90°]`, roll is the rotation about the laser
//! axis, positive when the right side goes down, in `(-180°, 180°]`.

use crate::math::{Mat3, Vec3, rad_to_deg, wrap_deg_360};

/// One computed survey leg direction.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Attitude {
    /// Bearing of the laser axis from magnetic north, degrees, `[0, 360)`.
    pub azimuth_deg: f64,
    /// Elevation of the laser axis, degrees, `[-90, 90]`.
    pub inclination_deg: f64,
    /// Rotation about the laser axis, degrees, `(-180, 180]`.
    pub roll_deg: f64,
    /// Magnetic dip (angle of the field below horizontal), degrees.
    pub dip_deg: f64,
}

/// World basis expressed in body coordinates, built from the two reference vectors.
///
/// Returns `None` when `g` or `m` is (near) zero or when they are parallel.
#[must_use]
pub fn world_basis(g: Vec3, m: Vec3) -> Option<Mat3> {
    let d = g.normalized()?;
    let m_hat = m.normalized()?;
    let n = (m_hat - d * m_hat.dot(d)).normalized()?;
    let e = d.cross(n);
    // Rows are N, E, D in body coordinates: `basis * v_body` gives `v_world`.
    Some(Mat3::from_rows(n.to_array(), e.to_array(), d.to_array()))
}

/// Direction of the body `X` axis (laser) in the world frame, unit vector `(n, e, d)`.
#[must_use]
pub fn laser_direction_world(g: Vec3, m: Vec3) -> Option<Vec3> {
    world_basis(g, m).map(|b| b * Vec3::X)
}

/// Full attitude from the gravity direction and the field vector (body frame, any units).
#[must_use]
pub fn attitude(g: Vec3, m: Vec3) -> Option<Attitude> {
    let basis = world_basis(g, m)?;
    let x_world = basis * Vec3::X;
    let azimuth_deg = wrap_deg_360(rad_to_deg(libm::atan2(x_world.y, x_world.x)));
    let inclination_deg = rad_to_deg(libm::asin(-x_world.z.clamp(-1.0, 1.0)));
    let g_hat = g.normalized()?;
    // `atan2(-0.0, negative)` is exactly −π; fold it onto +180 to honour `(-180, 180]`.
    let mut roll_deg = rad_to_deg(libm::atan2(g_hat.y, g_hat.z));
    if roll_deg <= -180.0 {
        roll_deg = 180.0;
    }
    let m_hat = m.normalized()?;
    let dip_deg = rad_to_deg(libm::asin(m_hat.dot(g_hat).clamp(-1.0, 1.0)));
    Some(Attitude {
        azimuth_deg,
        inclination_deg,
        roll_deg,
        dip_deg,
    })
}

/// Synthesises body-frame `g` and `m` for a given attitude. Used by the tests and by the solver
/// self-test; the inverse of [`attitude`].
///
/// `field` is the world-frame field `(north, 0, down)` with the wanted magnitude and dip.
#[must_use]
pub fn synthesize(azimuth_deg: f64, inclination_deg: f64, roll_deg: f64, field: Vec3) -> (Vec3, Vec3) {
    let a = crate::math::deg_to_rad(azimuth_deg);
    let i = crate::math::deg_to_rad(inclination_deg);
    let r = crate::math::deg_to_rad(roll_deg);
    let (sa, ca) = libm::sincos(a);
    let (si, ci) = libm::sincos(i);
    let (sr, cr) = libm::sincos(r);
    // Body axes in world coordinates.
    let x = Vec3::new(ci * ca, ci * sa, -si);
    let y0 = Vec3::new(-sa, ca, 0.0);
    let z0 = x.cross(y0);
    let y = y0 * cr + z0 * sr;
    let z = y0 * (-sr) + z0 * cr;
    let down = Vec3::Z;
    let g = Vec3::new(x.dot(down), y.dot(down), z.dot(down));
    let m = Vec3::new(x.dot(field), y.dot(field), z.dot(field));
    (g, m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(dip_deg: f64, magnitude: f64) -> Vec3 {
        let d = crate::math::deg_to_rad(dip_deg);
        Vec3::new(magnitude * d.cos(), 0.0, magnitude * d.sin())
    }

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn level_pointing_north() {
        let (g, m) = synthesize(0.0, 0.0, 0.0, field(60.0, 48.0));
        assert!(close(g.z, 1.0, 1e-12));
        let att = attitude(g, m).unwrap();
        assert!(close(att.azimuth_deg, 0.0, 1e-9) || close(att.azimuth_deg, 360.0, 1e-9));
        assert!(close(att.inclination_deg, 0.0, 1e-9));
        assert!(close(att.roll_deg, 0.0, 1e-9));
        assert!(close(att.dip_deg, 60.0, 1e-9));
    }

    #[test]
    fn roundtrip_grid() {
        let f = field(63.5, 48.2);
        for az in [0.0, 37.0, 90.0, 180.0, 270.0, 359.0] {
            for inc in [-80.0, -45.0, -3.0, 0.0, 12.5, 60.0, 85.0] {
                for roll in [-170.0, -90.0, -10.0, 0.0, 45.0, 90.0, 179.0] {
                    let (g, m) = synthesize(az, inc, roll, f);
                    let att = attitude(g, m).unwrap();
                    let d_az = (att.azimuth_deg - az + 540.0).rem_euclid(360.0) - 180.0;
                    assert!(close(d_az, 0.0, 1e-7), "az {az} inc {inc} roll {roll}: {att:?}");
                    assert!(close(att.inclination_deg, inc, 1e-7), "{att:?}");
                    assert!(close(att.roll_deg, roll, 1e-7), "{att:?}");
                    assert!(close(att.dip_deg, 63.5, 1e-7));
                }
            }
        }
    }

    #[test]
    fn roll_sign_right_side_down() {
        // Rolled 30° right: the right (+Y) axis dips, so gravity has a positive Y component.
        let (g, _) = synthesize(0.0, 0.0, 30.0, field(60.0, 48.0));
        assert!(g.y > 0.0);
        assert!(close(
            attitude(g, Vec3::new(1.0, 0.0, 0.0)).unwrap().roll_deg,
            30.0,
            1e-9
        ));
    }

    #[test]
    fn degenerate_inputs() {
        assert!(attitude(Vec3::ZERO, Vec3::X).is_none());
        assert!(attitude(Vec3::Z, Vec3::Z).is_none());
    }
}
