// ABOUTME: Shared 2D camera. World units are Doom's signed integer space;
// ABOUTME: zoom is pixels-per-world-unit. Y is flipped on the way to screen
// ABOUTME: because Doom's +y points north (up in-game) but iced's +y points down.

use glam::Vec2;

#[derive(Debug, Clone, Copy)]
pub struct Camera2D {
    pub center: Vec2,
    pub zoom: f32,
}

impl Default for Camera2D {
    fn default() -> Self {
        Self {
            center: Vec2::ZERO,
            zoom: 0.25,
        }
    }
}

impl Camera2D {
    pub fn world_to_screen(&self, world: Vec2, viewport: Vec2) -> Vec2 {
        let centered = world - self.center;
        let flipped = Vec2::new(centered.x, -centered.y);
        flipped * self.zoom + viewport * 0.5
    }

    pub fn screen_to_world(&self, screen: Vec2, viewport: Vec2) -> Vec2 {
        let centered = (screen - viewport * 0.5) / self.zoom;
        self.center + Vec2::new(centered.x, -centered.y)
    }

    /// Zoom about a screen-space pivot so that the world point under the cursor
    /// stays under the cursor after the zoom change.
    pub fn zoom_about(&mut self, screen_pivot: Vec2, viewport: Vec2, factor: f32) {
        let world_before = self.screen_to_world(screen_pivot, viewport);
        self.zoom = (self.zoom * factor).clamp(0.005, 32.0);
        let world_after = self.screen_to_world(screen_pivot, viewport);
        self.center += world_before - world_after;
    }

    pub fn pan_screen(&mut self, screen_delta: Vec2) {
        let world_delta = Vec2::new(screen_delta.x, -screen_delta.y) / self.zoom;
        self.center -= world_delta;
    }

    /// Frame the given world-space AABB inside the viewport with a small margin.
    pub fn frame_aabb(&mut self, min: Vec2, max: Vec2, viewport: Vec2) {
        let size = (max - min).max(Vec2::splat(1.0));
        let margin = 0.9;
        let zx = viewport.x / size.x;
        let zy = viewport.y / size.y;
        self.zoom = zx.min(zy) * margin;
        self.center = (min + max) * 0.5;
    }

    /// Power-of-two world-space grid spacing chosen so major lines stay around
    /// 64 px on screen at the current zoom level.
    pub fn grid_step(&self) -> f32 {
        let target_pixels = 64.0_f32;
        let world_per_pixel = 1.0 / self.zoom.max(1e-6);
        let raw = target_pixels * world_per_pixel;
        let exp = raw.log2().round();
        2.0_f32.powf(exp).max(1.0)
    }
}
