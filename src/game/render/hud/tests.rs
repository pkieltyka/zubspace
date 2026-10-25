use super::*;

#[test]
fn hud_max_chars_keeps_a_readable_floor() {
    assert_eq!(hud_max_chars(120.0, 8.0), HUD_MIN_CHARS);
    assert_eq!(hud_max_chars(320.0, 8.0), 40);
}

#[test]
fn wrap_hud_text_breaks_on_words_and_caps_long_tokens() {
    let lines = wrap_hud_text("Map bdhadar.lvl BOUNCE SHIELD SHUTDOWN SAFE", 18);
    assert_eq!(
        lines,
        vec!["Map bdhadar.lvl", "BOUNCE SHIELD", "SHUTDOWN SAFE"]
    );
    assert!(
        wrap_hud_text("averyverylongtoken", 8)
            .iter()
            .all(|line| line.len() <= 8)
    );
}

#[test]
fn notification_stack_uses_original_anchor_and_fits_reference_slots() {
    let desktop_origin = notification_origin(1280.0, 720.0);
    assert!((desktop_origin - vec2(240.0, 432.0)).length() < 0.001);
    assert!(notification_stack_inside(
        desktop_origin,
        360.0,
        NOTIFICATION_LIMIT,
        12.0,
        1280.0,
        720.0
    ));

    let compact_origin = notification_origin(360.0, 640.0);
    assert!((compact_origin - vec2(72.0, 384.0)).length() < 0.001);
    assert!(notification_stack_inside(
        compact_origin,
        240.0,
        NOTIFICATION_LIMIT,
        12.0,
        360.0,
        640.0
    ));
    assert!(!notification_stack_inside(
        compact_origin,
        320.0,
        NOTIFICATION_LIMIT,
        12.0,
        360.0,
        640.0
    ));
}

#[test]
fn notification_origin_does_not_panic_on_short_viewports() {
    let tiny = notification_origin(640.0, 200.0);
    assert!(tiny.x.is_finite() && tiny.y.is_finite());
    let extreme = notification_origin(0.0, 0.0);
    assert!(extreme.x.is_finite() && extreme.y.is_finite());
}

#[test]
fn notification_stack_shifts_inside_narrow_viewports() {
    let position = notification_stack_position(360.0, 640.0, 294.0, NOTIFICATION_LIMIT, 12.0);
    assert!((position - vec2(60.0, 384.0)).length() < 0.001);
    assert!(notification_stack_inside(
        position,
        294.0,
        NOTIFICATION_LIMIT,
        12.0,
        360.0,
        640.0
    ));
}
