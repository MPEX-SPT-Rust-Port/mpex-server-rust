//! Linear interpolation and range mapping, mirroring `Utils/MathUtil.cs`.

/// Maps a value from an input range to an output range linearly, mirroring `MathUtil.MapToRange`.
///
/// The result is clamped to `[min_out, max_out]`, so an `x` outside `[min_in, max_in]` saturates
/// rather than extrapolating.
pub fn map_to_range(x: f64, min_in: f64, max_in: f64, min_out: f64, max_out: f64) -> f64 {
    let delta_in = max_in - min_in;
    let delta_out = max_out - min_out;

    let x_scale = (x - min_in) / delta_in;

    (min_out + x_scale * delta_out).clamp(min_out, max_out)
}

/// Linear interpolation of `xp` over the support points `x`/`y`, mirroring `MathUtil.Interp1`.
///
/// An `xp` past either end of `x` clamps to the matching end of `y`. When no pair of support points
/// brackets `xp` the result is `0.0`, which is the C# `return default` at `MathUtil.cs:74`: `T` is
/// unconstrained there, so `T?` is annotation only and `default(double)` is what its callers
/// receive. Only a NaN reaches that branch — see `interp1_falls_through_to_zero`.
///
/// Deviation: C# throws on an empty `x` or a `y` shorter than `x`; both yield `0.0` here rather than
/// panicking across a future FFI boundary. A `y` longer than `x` is fine in both, and the above-max
/// path returns `y`'s true last element to match.
pub fn interp1(xp: f64, x: &[f64], y: &[f64]) -> f64 {
    if x.is_empty() || y.len() < x.len() {
        return 0.0;
    }

    if xp > x[x.len() - 1] {
        // Value is above max provided value in x array, clamp result to last value
        return y[y.len() - 1];
    }

    if xp < x[0] {
        // Value is below min provided value in x array, clamp result to first value
        return y[0];
    }

    for i in 0..x.len() - 1 {
        if xp >= x[i] && xp <= x[i + 1] {
            return y[i] + (xp - x[i]) * (y[i + 1] - y[i]) / (x[i + 1] - x[i]);
        }
    }

    0.0
}

#[cfg(test)]
mod tests {
    use super::{interp1, map_to_range};

    #[test]
    fn interp1_interpolates_midpoint() {
        assert_eq!(interp1(1.5, &[1.0, 2.0], &[10.0, 20.0]), 15.0);
    }

    #[test]
    fn interp1_matches_csharp_end_behaviour() {
        // `MathUtil.Interp1` clamps to the end of `y` on both sides rather than extrapolating or
        // returning null; the above-max case is pinned by `MathUtilTests.InterpTest`.
        assert_eq!(interp1(11.0, &[1.0, 10.0], &[2.0, 10.0]), 10.0);
        assert_eq!(interp1(0.0, &[1.0, 10.0], &[2.0, 10.0]), 2.0);
        // A longer `y` is legal in C#; above-max still yields `y[^1]`, y's true last element.
        assert_eq!(interp1(11.0, &[1.0, 10.0], &[2.0, 10.0, 99.0]), 99.0);
    }

    /// The `return default` branch of `MathUtil.cs:74` — `0.0`, not a sentinel.
    #[test]
    fn interp1_falls_through_to_zero() {
        // NaN loses every comparison, so no branch matches.
        assert_eq!(interp1(f64::NAN, &[1.0, 2.0], &[10.0, 20.0]), 0.0);
        // A NaN support point opens a gap no adjacent pair brackets. Note an unsorted-but-finite
        // `x` cannot reach here: past the two end clamps, `xp > x[i]` forces `xp > x[i + 1]` down
        // the whole chain, which contradicts `xp <= x[len - 1]`.
        assert_eq!(
            interp1(5.0, &[1.0, f64::NAN, 10.0], &[10.0, 20.0, 30.0]),
            0.0
        );
    }

    /// The `[TestCase]` vectors of `MathUtilTests.InterpTest`, expected values verbatim.
    #[test]
    fn interp1_matches_csharp_vectors() {
        assert_eq!(
            interp1(
                15.0,
                &[1.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0],
                &[
                    11000.0, 20000.0, 32000.0, 45000.0, 58000.0, 70000.0, 82000.0
                ],
            ),
            26000.0
        );
        assert_eq!(
            interp1(5.0, &[1.0, 10.0], &[0.0, 1000.0]),
            444.444_444_444_444_46
        );
        assert_eq!(
            interp1(12.0, &[1.0, 10.0, 500.0, 510.0], &[0.0, 10.0, 20.0, 30.0]),
            10.040_816_326_530_612
        );
        assert_eq!(
            interp1(1.0, &[1.0, 10.0, 500.0, 510.0], &[2.0, 10.0, 20.0, 30.0]),
            2.0
        );
    }

    /// C# would throw on these; the sanctioned deviation is `0.0` rather than a panic.
    #[test]
    fn interp1_yields_zero_for_malformed_support_points() {
        assert_eq!(interp1(1.0, &[], &[]), 0.0);
        assert_eq!(interp1(1.0, &[1.0, 2.0], &[10.0]), 0.0);
    }

    #[test]
    fn map_to_range_scales_and_clamps() {
        assert_eq!(map_to_range(5.0, 0.0, 10.0, 0.0, 100.0), 50.0);
        // Out of range on either side saturates at the output bounds.
        assert_eq!(map_to_range(20.0, 0.0, 10.0, 0.0, 100.0), 100.0);
        assert_eq!(map_to_range(-5.0, 0.0, 10.0, 0.0, 100.0), 0.0);
    }

    /// `MathUtilTests.MapToRangeTest`, expected value verbatim.
    #[test]
    fn map_to_range_matches_csharp_vector() {
        assert_eq!(map_to_range(0.5, 0.0, 1.0, 1.0, 3.0), 2.0);
    }
}
