use super::*;

#[test]
fn preferred_scale_survives_later_integer_events() {
    for integer_scale in [120, 240, 360] {
        assert_eq!(
            buffer_dimensions(800, 600, integer_scale, Some(180), true),
            (1200, 900, 1)
        );
    }
    assert_eq!(
        buffer_dimensions(800, 600, 240, Some(90), true),
        (600, 450, 1)
    );
    assert_eq!(
        buffer_dimensions(800, 600, 240, None, false),
        (1600, 1200, 2)
    );
}

#[test]
fn fractional_dimensions_round_half_up() {
    assert_eq!(scaled_dimension(1920, 180), 2880);
    assert_eq!(scaled_dimension(721, 180), 1082);
    assert_eq!(scaled_dimension(100, 120), 100);
}

#[test]
fn buffer_dimensions_bound_stride_and_memory() {
    assert!(valid_buffer_dimensions(7680, 4320));
    assert!(!valid_buffer_dimensions(0, 1080));
    assert!(!valid_buffer_dimensions(i32::MAX as u32, 1));
    assert!(!valid_buffer_dimensions(16_384, 16_384));
}
