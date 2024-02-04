use std::os::fd::OwnedFd;

use crate::terminal::Terminal;
use egui::{self, TextStyle, Vec2};

pub trait GetCharSize {
    fn get_char_size(&self, style: &TextStyle) -> Vec2;
}

impl GetCharSize for egui::Context {
    fn get_char_size(&self, style: &TextStyle) -> Vec2 {
        let font_id = self.style().text_styles[style].clone();
        self.fonts(|fonts| {
            let height = font_id.size;
            let layout = fonts.layout(
                "@".to_string(),
                font_id,
                egui::Color32::default(),
                f32::INFINITY,
            );

            Vec2::new(layout.mesh_bounds.width(), height)
        })
    }
}

pub struct TermGui<'a> {
    terminal: Terminal<'a>,
    char_size: Option<Vec2>,
}

impl<'a> TermGui<'a> {
    pub fn new(cc: &eframe::CreationContext<'_>, fd: OwnedFd) -> Self {
        cc.egui_ctx.style_mut(|style| {
            style.override_text_style = Some(TextStyle::Monospace);
        });
        Self {
            terminal: Terminal::new(fd),
            char_size: None,
        }
    }

    fn init(&mut self, ctx: &egui::Context) {
        self.char_size = Some(ctx.get_char_size(&TextStyle::Monospace));
    }
}

impl<'a> eframe::App for TermGui<'a> {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let None = self.char_size {
            self.init(ctx);
            println!("proportions: {:?}\n", self.char_size);
        }
        let Ok(()) = self.terminal.read() else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        };
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.input(|state| {
                for event in state.events.iter() {
                    let bytes = match event {
                        egui::Event::Key {
                            key: egui::Key::Enter,
                            pressed: true,
                            ..
                        } => b"\n".as_slice(),
                        egui::Event::Text(text) => text.as_bytes(),
                        _ => b"".as_slice(),
                    };
                    let Ok(_) = self.terminal.write(bytes) else {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        continue;
                    };
                }
            });

            let res = ui.label(self.terminal.buffer());

            let bottom = res.rect.bottom();
            let left = res.rect.left();
            let painter = ui.painter();
            let char_size = *self.char_size.as_ref().expect("char size to have been set");
            let cursor_cell_offset = self.terminal.char_to_cursor_offset();
            let cursor_offset = cursor_cell_offset * char_size;

            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::Pos2::new(left + cursor_offset.x, bottom + cursor_offset.y),
                    char_size.clone().into(),
                ),
                0.0,
                egui::Color32::GRAY,
            );
        });
    }
}
