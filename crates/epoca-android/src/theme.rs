/// Theme constants for the Android renderer.
/// Matches the default GPUI light theme.

#[derive(Debug, Clone, Copy)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }
}

pub struct Theme {
    pub bg: Color,
    pub text: Color,
    pub text_muted: Color,
    pub primary: Color,
    pub primary_text: Color,
    pub border: Color,
    pub input_bg: Color,
    pub button_bg: Color,
    pub font_size: f32,
    pub heading_size: f32,
    pub padding: f32,
    pub default_gap: f32,
    pub input_height: f32,
    pub button_pad_h: f32,
    pub button_pad_v: f32,
    pub border_radius: f32,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            bg: Color::new(1.0, 1.0, 1.0, 1.0),
            text: Color::new(0.1, 0.1, 0.1, 1.0),
            text_muted: Color::new(0.5, 0.5, 0.5, 1.0),
            primary: Color::new(0.2, 0.45, 0.9, 1.0),
            primary_text: Color::new(1.0, 1.0, 1.0, 1.0),
            border: Color::new(0.82, 0.82, 0.82, 1.0),
            input_bg: Color::new(0.96, 0.96, 0.96, 1.0),
            button_bg: Color::new(0.92, 0.92, 0.92, 1.0),
            font_size: 16.0,
            heading_size: 24.0,
            padding: 16.0,
            default_gap: 4.0,
            input_height: 40.0,
            button_pad_h: 12.0,
            button_pad_v: 8.0,
            border_radius: 6.0,
        }
    }
}
