//! Spherical geometry for v0.3 — pure functions, no host, no I/O.
//!
//! A point is `[lat_degrees, lon_degrees, altitude_m]`. Distances are metres on
//! the WGS84 mean sphere (the Haversine formula), which is within about 0.5% of
//! the true geodesic for the ranges a field script cares about — and unlike a
//! flat-earth approximation it stays right over kilometres.
//!
//! Altitude is deliberately *not* folded into `distance`; a horizontal distance
//! is what a geofence or a flight-plan check wants, and a script that needs the
//! 3-D length of a local offset says `length of (target - home)`.

use std::f64::consts::PI;

/// WGS84 mean radius, metres.
pub const EARTH_RADIUS_M: f64 = 6_371_008.8;

/// Great-circle distance in metres between two lat/lon points.
pub fn haversine_m(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (lat1, lon1) = (a[0].to_radians(), a[1].to_radians());
    let (lat2, lon2) = (b[0].to_radians(), b[1].to_radians());
    let dlat = lat2 - lat1;
    let dlon = lon2 - lon1;
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * h.sqrt().min(1.0).asin()
}

/// Initial bearing in degrees (0 = north, 90 = east) from `a` to `b`.
pub fn bearing_deg(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (lat1, lat2) = (a[0].to_radians(), b[0].to_radians());
    let dlon = (b[1] - a[1]).to_radians();
    let y = dlon.sin() * lat2.cos();
    let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * dlon.cos();
    normalise_deg(y.atan2(x).to_degrees())
}

/// The compass bearing of a local vector, degrees clockwise from north.
///
/// A Kaze vector is `[x, y, z]` in a local frame: x east, y north, z up. This is
/// what `bearing to v` means — the direction the vector points, not the direction
/// to a place.
pub fn bearing_of(v: [f64; 3]) -> f64 {
    normalise_deg(v[0].atan2(v[1]).to_degrees())
}

/// Euclidean length of a vector, in whatever unit its components are in.
pub fn magnitude(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

fn normalise_deg(deg: f64) -> f64 {
    let d = deg % 360.0;
    if d < 0.0 {
        d + 360.0
    } else {
        d
    }
}

/// Degrees to radians. `x in radians` in the language.
pub fn to_radians(deg: f64) -> f64 {
    deg * PI / 180.0
}

/// Radians to degrees. `x in degrees` in the language.
pub fn to_degrees(rad: f64) -> f64 {
    rad * 180.0 / PI
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tolerance: f64) -> bool {
        (a - b).abs() <= tolerance
    }

    #[test]
    fn one_degree_of_latitude_is_about_111_km() {
        let d = haversine_m([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        assert!(close(d, 111_194.9, 50.0), "{d}");
    }

    #[test]
    fn one_degree_of_longitude_shrinks_with_latitude() {
        let equator = haversine_m([0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let zurich = haversine_m([47.6, 8.5, 0.0], [47.6, 9.5, 0.0]);
        assert!(close(equator, 111_194.9, 50.0), "{equator}");
        assert!(zurich < equator, "{zurich} should be shorter than {equator}");
        assert!(close(zurich, 75_000.0, 2_000.0), "{zurich}");
    }

    #[test]
    fn a_short_hop_across_a_swiss_field_is_tens_of_metres() {
        let d = haversine_m([47.6000, 8.5000, 0.0], [47.6004, 8.5004, 0.0]);
        assert!(close(d, 53.4, 1.0), "{d}");
    }

    #[test]
    fn the_same_point_is_zero_apart() {
        let p = [26.1445, 91.7362, 50.0];
        assert_eq!(haversine_m(p, p), 0.0);
    }

    #[test]
    fn bearing_has_the_cardinal_directions_right() {
        let origin = [0.0, 0.0, 0.0];
        assert!(close(bearing_deg(origin, [1.0, 0.0, 0.0]), 0.0, 0.01));
        assert!(close(bearing_deg(origin, [0.0, 1.0, 0.0]), 90.0, 0.01));
        assert!(close(bearing_deg(origin, [-1.0, 0.0, 0.0]), 180.0, 0.01));
        assert!(close(bearing_deg(origin, [0.0, -1.0, 0.0]), 270.0, 0.01));
    }

    #[test]
    fn a_northbound_bearing_never_reports_360() {
        // atan2 can land a hair below zero; the answer is still north.
        let b = bearing_deg([10.0, 10.0, 0.0], [10.1, 10.0, 0.0]);
        assert!(b == 0.0 || b >= 0.0, "{b}");
        assert!(b < 360.0, "{b}");
        let north = bearing_of([0.0, 5.0, 0.0]);
        assert!(close(north, 0.0, 0.001), "{north}");
    }

    #[test]
    fn a_local_vector_has_a_compass_bearing() {
        assert!(close(bearing_of([1.0, 0.0, 0.0]), 90.0, 0.001), "east");
        assert!(close(bearing_of([0.0, 1.0, 0.0]), 0.0, 0.001), "north");
        assert!(close(bearing_of([-3.0, -4.0, 0.0]), 216.87, 0.01), "south-west");
    }

    #[test]
    fn magnitude_is_the_3d_length() {
        assert_eq!(magnitude([3.0, 4.0, 0.0]), 5.0);
        assert_eq!(magnitude([0.0, 0.0, 0.0]), 0.0);
        assert!(close(magnitude([1.0, 2.0, 2.0]), 3.0, 1e-9));
    }

    #[test]
    fn a_degree_is_pi_over_180_radians() {
        assert!(close(to_radians(180.0), PI, 1e-12));
        assert!(close(to_degrees(PI), 180.0, 1e-12));
        assert!(close(to_degrees(to_radians(37.5)), 37.5, 1e-12));
    }
}
