use macroquad::prelude::*;

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    fn zub_loading_progress(ptr: *const u8, len: i32, loaded: i32, total: i32);
    fn zub_loading_done();
}

pub(super) async fn draw_loading_frame(stage: &str, loaded: usize, total: usize) {
    #[cfg(target_arch = "wasm32")]
    {
        update_browser_loading_progress(stage, loaded, total);
        next_frame().await;
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        clear_background(Color::from_rgba(5, 5, 8, 255));

        let width = screen_width().clamp(220.0, 420.0);
        let height = screen_height();
        let x = (screen_width() - width) * 0.5;
        let y = height * 0.5 - 16.0;
        let progress = if total == 0 {
            0.0
        } else {
            loaded as f32 / total as f32
        };

        draw_text(
            "zubspace",
            x,
            y - 28.0,
            28.0,
            Color::from_rgba(230, 236, 244, 255),
        );
        draw_rectangle_lines(x, y, width, 8.0, 1.0, Color::from_rgba(95, 116, 135, 255));
        draw_rectangle(
            x + 1.0,
            y + 1.0,
            (width - 2.0) * progress.clamp(0.0, 1.0),
            6.0,
            Color::from_rgba(86, 206, 226, 255),
        );
        draw_text(
            &format!("{stage} {loaded}/{total}"),
            x,
            y + 34.0,
            18.0,
            Color::from_rgba(160, 176, 190, 255),
        );

        next_frame().await;
    }
}

#[cfg(target_arch = "wasm32")]
fn update_browser_loading_progress(stage: &str, loaded: usize, total: usize) {
    let Ok(stage_len) = i32::try_from(stage.len()) else {
        return;
    };
    let Ok(loaded) = i32::try_from(loaded) else {
        return;
    };
    let Ok(total) = i32::try_from(total) else {
        return;
    };
    unsafe {
        zub_loading_progress(stage.as_ptr(), stage_len, loaded, total);
    }
}

pub(crate) fn hide_loading_overlay() {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        zub_loading_done();
    }
}

pub(super) fn should_present_loading_progress(loaded: usize, total: usize) -> bool {
    loaded == total || loaded.is_multiple_of(4)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn set_loading_status(stage: &str) {
    update_browser_loading_progress(stage, 0, 0);
}
