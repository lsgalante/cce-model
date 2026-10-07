//! Lengths as the HUD says them: in millimetres, or metres once they reach a
//! metre, when the file's format says what its units are; as bare numbers
//! when it does not (OBJ, PLY), since calling those millimetres would be a
//! guess presented as a measurement.

use cce_mesh_io::Unit;
use glam::Vec3;

/// The smallest of 1, 2 or 5 × 10ⁿ that is at least `raw`: a step a ruler
/// would use.
pub fn nice_step(raw: f32) -> f32 {
    if !(raw > 0.0) || !raw.is_finite() {
        return 1.0;
    }
    // In f64, so 5 × 10⁻² comes out as the f32 nearest 0.05, not a hair
    // under it: the grid's lines are whole multiples of this.
    let raw = raw as f64;
    let decade = 10f64.powi(raw.log10().floor() as i32);
    for m in [1.0, 2.0, 5.0] {
        if m * decade >= raw * (1.0 - 1e-6) {
            return (m * decade) as f32;
        }
    }
    (10.0 * decade) as f32
}

/// About three significant figures, without trailing zeros.
fn number(v: f32) -> String {
    let s = match v.abs() {
        a if a >= 100.0 => format!("{v:.0}"),
        a if a >= 10.0 => format!("{v:.1}"),
        a if a >= 1.0 => format!("{v:.2}"),
        _ => format!("{v:.3}"),
    };
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// The unit suffix and divisor for lengths up to `largest` file units.
fn scale(largest: f32, unit: Unit) -> (&'static str, f32) {
    match unit.millimetres() {
        Some(mm) if largest * mm >= 1000.0 => (" m", mm / 1000.0),
        Some(mm) => (" mm", mm),
        None => ("", 1.0),
    }
}

/// One length in file units, as said.
pub fn length(v: f32, unit: Unit) -> String {
    let (suffix, k) = scale(v, unit);
    format!("{}{suffix}", number(v * k))
}

/// A box's width × height × depth, all in the unit its largest side needs.
pub fn dimensions(size: Vec3, unit: Unit) -> String {
    let (suffix, k) = scale(size.max_element(), unit);
    format!("{} × {} × {}{suffix}", number(size.x * k), number(size.y * k), number(size.z * k))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_are_one_two_five() {
        assert_eq!(nice_step(10.0), 10.0);
        assert_eq!(nice_step(11.0), 20.0);
        assert_eq!(nice_step(3.0), 5.0);
        assert_eq!(nice_step(0.07), 0.1);
        assert_eq!(nice_step(0.0), 1.0);
    }

    #[test]
    fn lengths_say_their_unit_only_when_the_format_does() {
        assert_eq!(length(10.0, Unit::Millimetre), "10 mm");
        assert_eq!(length(0.5, Unit::Metre), "500 mm");
        assert_eq!(length(2.5, Unit::Metre), "2.5 m");
        assert_eq!(length(0.25, Unit::Unspecified), "0.25");
        assert_eq!(dimensions(Vec3::new(120.0, 45.26, 80.0), Unit::Millimetre), "120 × 45.3 × 80 mm");
        assert_eq!(dimensions(Vec3::new(1.8, 0.75, 0.9), Unit::Metre), "1.8 × 0.75 × 0.9 m");
        assert_eq!(dimensions(Vec3::new(2.0, 1.0, 0.5), Unit::Unspecified), "2 × 1 × 0.5");
    }
}
