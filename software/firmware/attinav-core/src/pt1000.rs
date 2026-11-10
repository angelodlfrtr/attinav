//! Coil thermometers: Pt1000 in a divider read by the MCU ADC.
//!
//! Circuit (mag sheet, one per triad): `+3V3 ── 10 kΩ 0.1 % ── TEMP_MAGx (ADC) ── Pt1000 ── GND`.
//! The ADC reference is `VDDA = +3.3VA`, about 60 mV below `+3V3`, so the divider is *not*
//! ratiometric to the reference: the firmware measures `VDDA` through `VREFINT` and uses the
//! nominal `+3V3` for the divider top. The residual error (LDO tolerance ≈ 1 %) is ≈ 0.7 °C of
//! absolute offset, irrelevant for a gain-tempco model that is itself fitted against this reading.

/// Divider description.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Divider {
    /// Series resistor from the supply to the ADC node, in ohms.
    pub r_series_ohm: f64,
    /// Supply at the top of the divider, in volts.
    pub v_supply: f64,
}

impl Divider {
    /// The Rev B divider.
    pub const REV_B: Self = Self {
        r_series_ohm: 10_000.0,
        v_supply: 3.3,
    };

    /// Resistance of the RTD from the node voltage, or `None` if the node is at or above the supply
    /// (open RTD) or at zero (shorted RTD).
    #[must_use]
    pub fn resistance_ohm(&self, v_node: f64) -> Option<f64> {
        if v_node <= 0.0 || v_node >= self.v_supply {
            return None;
        }
        Some(self.r_series_ohm * v_node / (self.v_supply - v_node))
    }
}

/// Converts an ADC reading to the node voltage.
///
/// `vdda_v` is the measured analogue supply (= the ADC reference), `full_scale` the maximum code
/// of the configured resolution (`2^n - 1`).
#[must_use]
pub fn adc_to_volts(raw: u32, full_scale: u32, vdda_v: f64) -> f64 {
    f64::from(raw) * vdda_v / f64::from(full_scale)
}

/// Pt1000 resistance (ohms) to temperature (°C) through the inverse Callendar–Van Dusen equation.
///
/// IEC 60751 coefficients. The quadratic inverse is exact above 0 °C; below 0 °C the cubic `C`
/// term is neglected, which costs under 0.1 °C down to −40 °C — well inside what a coil tempco
/// model needs.
#[must_use]
pub fn resistance_to_celsius(r_ohm: f64) -> f64 {
    const R0: f64 = 1000.0;
    const A: f64 = 3.9083e-3;
    const B: f64 = -5.775e-7;
    let ratio = r_ohm / R0;
    let disc = A * A - 4.0 * B * (1.0 - ratio);
    (-A + libm::sqrt(disc.max(0.0))) / (2.0 * B)
}

/// Full chain: raw ADC code → °C, `None` if the RTD looks open or shorted.
#[must_use]
pub fn adc_to_celsius(raw: u32, full_scale: u32, vdda_v: f64, divider: &Divider) -> Option<f64> {
    let v = adc_to_volts(raw, full_scale, vdda_v);
    divider.resistance_ohm(v).map(resistance_to_celsius)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callendar_van_dusen_fixed_points() {
        assert!(resistance_to_celsius(1000.0).abs() < 1e-9);
        assert!((resistance_to_celsius(1385.055) - 100.0).abs() < 0.01);
        assert!((resistance_to_celsius(1077.935) - 20.0).abs() < 0.01);
        // -20 °C is 921.60 Ω with the cubic term; we are within the stated 0.1 °C.
        assert!((resistance_to_celsius(921.60) + 20.0).abs() < 0.1);
    }

    #[test]
    fn divider_roundtrip() {
        let d = Divider::REV_B;
        let v = 3.3 * 1000.0 / 11_000.0;
        assert!((d.resistance_ohm(v).unwrap() - 1000.0).abs() < 1e-9);
        assert!(d.resistance_ohm(0.0).is_none());
        assert!(d.resistance_ohm(3.3).is_none());
    }

    #[test]
    fn full_chain_at_room_temperature() {
        // 14-bit ADC, VDDA = 3.24 V, node at 20 °C: V = 3.3 * 1077.935 / 11077.935 = 0.32110 V.
        let v: f64 = 3.3 * 1077.935 / 11_077.935;
        let raw = (v / 3.24_f64 * 16383.0).round() as u32;
        let t = adc_to_celsius(raw, 16383, 3.24, &Divider::REV_B).unwrap();
        assert!((t - 20.0).abs() < 0.2, "t = {t}");
    }
}
