use super::uisupport::*;
use super::{UiCtx, RegHighlight};
use imgui::*;

pub enum RegisterSize<'a> {
    Reg8(&'a mut u8),
    Reg16(&'a mut u16),
    Reg32(&'a mut u32),
    Reg64(&'a mut u64),
}

/// A trait for an object that can display register contents to
/// a debugger view.
pub trait RegisterView {
    const WINDOW_SIZE: (f32, f32);
    const COLUMNS: usize;
    fn name<'a>(&'a self) -> &'a str;
    fn cpu_name<'a>(&'a self) -> &'a str;
    fn visit_regs<'s, F>(&'s mut self, col: usize, visit: F)
    where
        F: for<'a> FnMut(&'a str, RegisterSize<'a>, Option<&str>);
}

const COLOR_BG_NORMAL: [f32; 4] = [41.0/255.0,74.0/255.0,122.0/255.0,138.0/255.0];
const COLOR_BG_INPUT: [f32; 4] = [86.0/255.0,171.0/255.0,60.0/255.0,138.0/255.0];
const COLOR_BG_OUTPUT: [f32; 4] = [204.0/255.0,61.0/255.0,61.0/255.0,138.0/255.0];

pub(crate) fn render_regview<'a, 'ui, RV: RegisterView>(
    ui: &'a Ui<'ui>,
    ctx: &mut UiCtx,
    v: &mut RV,
) {
    let disasm = ctx.disasm.get(v.cpu_name());
    ui.window(im_str!("[{}] Registers", v.name()))
        .size(RV::WINDOW_SIZE, ImGuiCond::FirstUseEver)
        .build(|| {
            // Iterate on all the columns
            ui.columns(RV::COLUMNS as _, im_str!("columns"), true);
            for col in 0..RV::COLUMNS {
                // Visit regs for this column
                v.visit_regs(col, |name, val, desc| {
                    use self::RegisterSize::*;

                    // Check if this register requires some special
                    // highlight.
                    let bgcolor = match disasm {
                        None => COLOR_BG_NORMAL,
                        Some(d) => match d.regs_highlight.get(name) {
                            None => COLOR_BG_NORMAL,
                            Some(RegHighlight::Input) => COLOR_BG_INPUT,
                            Some(RegHighlight::Output) => COLOR_BG_OUTPUT,
                        }
                    };

                    // Draw the register box
                    let name = im_str!("{}", name);
                    ui.with_color_var(ImGuiCol::FrameBg, bgcolor, || {
                        match val {
                            Reg8(v) => imgui_input_hex(ui, name, v, true),
                            Reg16(v) => imgui_input_hex(ui, name, v, true),
                            Reg32(v) => imgui_input_hex(ui, name, v, true),
                            Reg64(v) => imgui_input_hex(ui, name, v, true),
                        };
                        if let Some(desc) = desc {
                            ui.text(im_str!("{}", desc));
                        }
                    });
                });
                ui.next_column();
            }
        });
}
