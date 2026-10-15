//! The AI's pure pieces (the decisions themselves run against captures: `tests/poc5.rs`).

use super::*;

#[test]
fn float_compare_is_csharps() {
    assert_eq!(float_compare(1.0, 2.0), -1);
    assert_eq!(float_compare(2.0, 1.0), 1);
    assert_eq!(float_compare(0.0, -0.0), 0);
    // `float.CompareTo`: NaN is less than every number and equal to itself.
    assert_eq!(float_compare(f32::NAN, -1e30), -1);
    assert_eq!(float_compare(-1e30, f32::NAN), 1);
    assert_eq!(float_compare(f32::NAN, f32::NAN), 0);
}

#[test]
fn targets_sort_by_weight_descending_and_keep_ties_in_order() {
    // `TargetInfo.CompareTo`: `other.Weight.CompareTo(Weight)`; up to 16 targets the introsort is an
    // insertion sort, which keeps equal weights in list order.
    let mut v = vec![(0, 1e6f32), (1, 1e6 + 1.0), (2, 1e6), (3, -1e6), (4, 1e6 + 1.0)];
    let cap = crate::csharp::sort::grown_capacity(v.len());
    crate::csharp::introsort(&mut v, cap, &mut |a, b| float_compare(b.1, a.1));
    assert_eq!(v.iter().map(|x| x.0).collect::<Vec<_>>(), [1, 4, 0, 2, 3]);
}

#[test]
fn scores_keep_decimal_scales() {
    // `0.8 * (decimal)0f` is "0.0", the score a ComplexConsideration that fails leaves.
    let s = mul(dec(0.8).unwrap(), dec(0.0).unwrap()).unwrap();
    assert_eq!(s.to_string(), "0.0");
    // The not-priority factor and the sleeping factor keep every digit.
    let s = mul(mul(dec(0.8).unwrap(), Decimal::lit(1, 8)).unwrap(), Decimal::lit(1, 3)).unwrap();
    assert_eq!(s.to_string(), "0.000000000008");
    // A cast action's base 2 times a threat consideration's 0 is "0" (`poc5-tb-121`'s second
    // decision); times its 1, "2".
    assert_eq!(mul(dec(2.0).unwrap(), dec(0.0).unwrap()).unwrap().to_string(), "0");
    assert_eq!(mul(dec(2.0).unwrap(), dec(1.0).unwrap()).unwrap().to_string(), "2");
}

#[test]
fn mathf_lerps_are_unitys() {
    // `InverseLerp`: clamped, 0 for an empty interval.
    assert_eq!(inverse_lerp(0.0, 1.0, 0.0).to_bits(), 0.0f32.to_bits());
    assert_eq!(inverse_lerp(0.0, 1.0, 1.0).to_bits(), 1.0f32.to_bits());
    assert_eq!(inverse_lerp(0.0, 1.0, 3.0).to_bits(), 1.0f32.to_bits());
    assert_eq!(inverse_lerp(7.5, 8.0, 7.75).to_bits(), 0.5f32.to_bits());
    assert_eq!(inverse_lerp(7.5, 8.0, 1.0).to_bits(), 0.0f32.to_bits());
    assert_eq!(inverse_lerp(2.0, 2.0, 5.0).to_bits(), 0.0f32.to_bits());
    // `LerpUnclamped` extrapolates; `Lerp` clamps `t`.
    assert_eq!(lerp_unclamped(1.0, 0.0, 2.0).to_bits(), (-1.0f32).to_bits());
    assert_eq!(lerp(1.0, 0.0, 2.0).to_bits(), 0.0f32.to_bits());
    assert_eq!(lerp(1.0, 0.0, 0.25).to_bits(), 0.75f32.to_bits());
    // The quotient is stored as a `float` before `Clamp01`: computed in double, rounded once.
    let x = inverse_lerp(0.0, 0.3, 0.1);
    assert_eq!(x.to_bits(), store_f32(f64::from(0.1f32) / f64::from(0.3f32)).to_bits());
}

#[test]
fn a_cast_actions_range_is_its_abilitys_in_meters() {
    // `40.Feet().Meters`: `40f * 0.3048f` in double, stored: the recorded 12.192 of the giant
    // centipede's spit (`poc5-tb-121`).
    let meters = store_f32(40.0 * f64::from(0.3048f32));
    assert_eq!(meters.to_bits(), 12.192f32.to_bits());
}
