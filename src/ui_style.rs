//! Shared visual tokens for the workspace and drawing editor.
use eframe::egui::{self, Color32, CornerRadius, Stroke};

pub const ACCENT: Color32 = Color32::from_rgb(112, 68, 220);
pub const SURFACE: Color32 = Color32::WHITE;
pub const WORKSPACE: Color32 = Color32::from_rgb(235, 237, 243);
pub const BORDER: Color32 = Color32::from_rgb(216, 220, 231);
pub const MUTED: Color32 = Color32::from_rgb(102, 115, 138);
pub const TEXT: Color32 = Color32::from_rgb(41, 49, 68);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Theme {
    #[default]
    Light,
    Dark,
}

pub fn workspace(theme: Theme) -> Color32 {
    match theme {
        Theme::Light => WORKSPACE,
        Theme::Dark => Color32::from_rgb(29, 30, 38),
    }
}

pub fn install(ctx: &egui::Context) {
    install_theme(ctx, Theme::Light);
}

pub fn install_theme(ctx: &egui::Context, theme: Theme) {
    let mut style = (*ctx.style()).clone();
    let dark = theme == Theme::Dark;
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    let surface = if dark {
        Color32::from_rgb(43, 44, 54)
    } else {
        SURFACE
    };
    let inset = if dark {
        Color32::from_rgb(34, 35, 43)
    } else {
        Color32::from_rgb(247, 248, 251)
    };
    let border = if dark {
        Color32::from_rgb(74, 75, 89)
    } else {
        BORDER
    };
    let text = if dark {
        Color32::from_rgb(238, 238, 245)
    } else {
        TEXT
    };
    let muted = if dark {
        Color32::from_rgb(183, 184, 200)
    } else {
        MUTED
    };
    let selected = if dark {
        Color32::from_rgb(68, 55, 104)
    } else {
        Color32::from_rgb(235, 235, 250)
    };
    visuals.panel_fill = surface;
    visuals.window_fill = surface;
    visuals.extreme_bg_color = inset;
    visuals.faint_bg_color = inset;
    visuals.weak_text_color = Some(muted);
    visuals.hyperlink_color = ACCENT;
    visuals.selection.bg_fill = selected;
    visuals.selection.stroke = Stroke::new(
        1.5,
        if dark {
            Color32::from_rgb(205, 190, 255)
        } else {
            ACCENT
        },
    );
    visuals.window_corner_radius = CornerRadius::same(8);
    visuals.menu_corner_radius = CornerRadius::same(7);
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = CornerRadius::same(6);
        widget.fg_stroke.color = text;
        widget.bg_stroke = Stroke::new(1.0, border);
    }
    visuals.widgets.inactive.weak_bg_fill = surface;
    visuals.widgets.hovered.weak_bg_fill = if dark {
        Color32::from_rgb(62, 62, 76)
    } else {
        Color32::from_rgb(241, 242, 250)
    };
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    visuals.widgets.active.weak_bg_fill = selected;
    visuals.widgets.active.bg_stroke = Stroke::new(2.0, ACCENT);
    visuals.widgets.open.weak_bg_fill = selected;
    style.spacing.button_padding = egui::vec2(8.0, 5.0);
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.visuals = visuals;
    ctx.set_style(style);
}

/// A primary action with distinct hover, focus, and disabled states.
pub fn primary(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    ui.scope(|ui| {
        let visuals = &mut ui.style_mut().visuals.widgets;
        visuals.inactive.weak_bg_fill = ACCENT;
        visuals.inactive.bg_stroke = Stroke::new(1.0, ACCENT);
        visuals.hovered.weak_bg_fill = Color32::from_rgb(88, 51, 184);
        visuals.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(88, 51, 184));
        visuals.active.weak_bg_fill = Color32::from_rgb(88, 51, 184);
        visuals.active.bg_stroke = Stroke::new(2.0, TEXT);
        ui.add_enabled(
            enabled,
            egui::Button::new(egui::RichText::new(label).color(Color32::WHITE).strong()),
        )
    })
    .inner
}
