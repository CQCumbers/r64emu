use imgui::*;
use imgui_sys;
use sdl2::keyboard::Scancode;

use super::decoding::DecodedInsn;
use super::uisupport::*;
use super::{RegHighlight, TraceEvent, UiCommand, UiCtx};

use std::time::Instant;

/// A trait for an object that can display register contents to
/// a debugger view.
pub trait DisasmView {
    /// Return the name of this object. The name will be composed
    /// as "\[NAME\] Disassembly".
    fn name(&self) -> &str;

    /// Return the current program counter.
    fn pc(&self) -> u64;

    /// "Mask" a PC value to consttain it within the currently-valid
    /// range. The disassembly view will only display opcodes within
    /// the range [pc_mask(0)..pc_mask(u64::max_value())].
    ///
    /// Most architectures need to implement this function to constraint
    /// the range within the actual bus size. For instance, a 8-bit architecture
    /// with a 14 bit memory bus might want to implement this masking the value
    /// with 0x3FFF.
    fn pc_mask(&self, pc: u64) -> u64;

    /// Disassemble a single instruction at the specified program counter;
    /// Returns the bytes composing the instruction and the string representation.
    fn disasm_block<Func: FnMut(u64, &[u8], &DecodedInsn)>(&self, pc_range: (u64, u64), f: Func);
}

struct ByteBuf<'a>(&'a [u8]);

impl<'a> std::fmt::LowerHex for ByteBuf<'a> {
    fn fmt(&self, fmtr: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        for byte in self.0 {
            fmtr.write_fmt(format_args!("{:02x}", byte))?;
        }
        Ok(())
    }
}

fn color(r: usize, g: usize, b: usize) -> [f32; 4] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
}

pub(crate) fn render_disasmview<'a, 'ui, DV: DisasmView>(
    ui: &'a Ui<'ui>,
    ctx: &mut UiCtx,
    v: &mut DV,
) {
    let cpu_name = v.name().to_owned();
    let cur_pc = v.pc();
    let mut set_command: Option<UiCommand> = None;
    let dctx = ctx.disasm.get_mut(&cpu_name).unwrap();

    // If we were asked to show a certain PC, then also get focus
    // as the user probably wants to see this window.
    if dctx.force_pc.is_some() {
        unsafe {
            imgui_sys::igSetNextWindowFocus();
        }
    }

    // Process current event (if any)
    match ctx.event {
        Some((ref evt, _)) => match **evt {
            TraceEvent::Breakpoint(ref bp_cpu_name, _, bp_pc) if *bp_cpu_name == cpu_name => {
                // Center breakpoint PC
                dctx.force_pc = Some(bp_pc);

                // Focus this window
                unsafe {
                    imgui_sys::igSetNextWindowFocus();
                }

                dctx.cursor_pc = None;

                // Start blinking effect
                dctx.blink_pc = Some((bp_pc, Instant::now()));
            }
            TraceEvent::WatchpointRead(ref bp_cpu_name, _)
            | TraceEvent::WatchpointWrite(ref bp_cpu_name, _)
                if *bp_cpu_name == cpu_name =>
            {
                // Center breakpoint PC
                dctx.force_pc = Some(cur_pc);

                // Focus this window
                unsafe {
                    imgui_sys::igSetNextWindowFocus();
                }

                dctx.cursor_pc = None;

                // Start blinking effect
                dctx.blink_pc = Some((cur_pc, Instant::now()));
            }
            TraceEvent::BreakpointOneShot(ref bp_cpu_name, bp_pc) if *bp_cpu_name == cpu_name => {
                // Center breakpoint PC
                dctx.force_pc = Some(bp_pc);

                // Focus this window
                unsafe {
                    imgui_sys::igSetNextWindowFocus();
                }

                dctx.blink_pc = None;
                dctx.cursor_pc = None;
            }
            TraceEvent::Stepped() | TraceEvent::Paused() | TraceEvent::GenericBreak(_) => {
                dctx.force_pc = Some(cur_pc);
                dctx.blink_pc = None;
                dctx.cursor_pc = None;
            }
            _ => {}
        },
        None => {}
    };

    Window::new(&im_str!("[{}] Disassembly", cpu_name))
        .size([450.0, 400.0], Condition::FirstUseEver)
        .build(ui, || {
            let has_focus =
                ui.is_window_focused_with_flags(WindowFocusedFlags::ROOT_AND_CHILD_WINDOWS);

            // *******************************************
            // Goto popup
            // *******************************************
            ui.popup(im_str!("###goto"), || {
                let mut s = ImString::new("00000000");
                ui.text(im_str!("Insert PC:"));
                if ui
                    .input_text(im_str!("###goto#input"), &mut s)
                    .chars_hexadecimal(true)
                    .enter_returns_true(true)
                    .auto_select_all(true)
                    .build()
                {
                    dctx.force_pc = u64::from_str_radix(s.as_ref(), 16).ok();
                    ui.close_current_popup();
                }
            });

            // *******************************************
            // Cursor input
            // *******************************************
            if has_focus {
                if ui.is_key_pressed(Scancode::Up as _) {
                    let cpc = match dctx.cursor_pc {
                        Some(cpc) => cpc - 4,
                        None => cur_pc - 4,
                    };
                    dctx.cursor_pc = Some(cpc);
                }
                if ui.is_key_pressed(Scancode::Down as _) {
                    let cpc = match dctx.cursor_pc {
                        Some(cpc) => cpc + 4,
                        None => cur_pc + 4,
                    };
                    dctx.cursor_pc = Some(cpc);
                }
            }

            // *******************************************
            // Button toolbar
            // *******************************************
            if ui.small_button(im_str!("Goto")) {
                ui.open_popup(im_str!("###goto"));
            }
            ui.same_line(0.0);
            if ui.small_button(im_str!("Center"))
                || (has_focus && ui.is_key_pressed(Scancode::C as _))
            {
                dctx.force_pc = Some(cur_pc);
            }
            ui.same_line(0.0);
            if ui.small_button(im_str!("Step"))
                || (has_focus && ui.is_key_pressed(Scancode::S as _))
            {
                set_command = Some(UiCommand::CpuStep(cpu_name.clone()));
            }
            ui.same_line(0.0);
            if ui.small_button(im_str!("Here"))
                || (has_focus && ui.is_key_pressed(Scancode::Return as _))
            {
                if let Some(cpc) = dctx.cursor_pc {
                    set_command = Some(UiCommand::BreakpointOneShot(cpu_name.clone(), cpc));
                }
            }
            ui.separator();

            // *******************************************
            // Main scroll view with disasm
            // *******************************************
            ChildWindow::new(&im_str!("###scrolling"))
                .size([0.0, 0.0])
                .always_vertical_scrollbar(true)
                .build(ui, || {
                    // Get the full extent of PC. Notice that the range is *inclusive*.
                    let mut pc_range = (v.pc_mask(u64::min_value()), v.pc_mask(u64::max_value()));

                    // Calculate a range of PC that will be used in the disasm
                    // view, that could be smaller than the full extent. We select
                    // up to 1M lines around the current PC.
                    // Notice that this is the full range of the listbox, not just
                    // the display range.
                    const MAX_LINES: u64 = 1024 * 1024;
                    pc_range.0 =
                        (cur_pc.saturating_sub(4 * MAX_LINES / 2) / 1024 * 1024).max(pc_range.0);
                    pc_range.1 = pc_range.0.saturating_add(4 * MAX_LINES - 1).min(pc_range.1);
                    let num_lines = (pc_range.1 - pc_range.0 + 1) / 4;

                    // Check if we were asked to scroll to a specific PC.
                    if let Some(force_pc) = dctx.force_pc {
                        let size = ui.content_region_avail();
                        let row_height = ui.text_line_height_with_spacing();
                        let scroll_y = ui.scroll_y();
                        let force_pc = v.pc_mask(force_pc);

                        let first_pc = pc_range
                            .0
                            .saturating_add((scroll_y / row_height) as u64 * 4);
                        let last_pc = first_pc.saturating_add((size[1] / row_height) as u64 * 4);

                        if force_pc < first_pc.saturating_add(4 * 4)
                            || force_pc > last_pc.saturating_sub(4 * 4)
                        {
                            let start_pc = force_pc
                                .saturating_sub(10 * 4)
                                .max(pc_range.0)
                                .min(pc_range.1);
                            ui.set_scroll_y(row_height * ((start_pc - pc_range.0) / 4) as f32);
                        }
                    }

                    // Display the non-clipped part of the listbox
                    let blink_pc = dctx.blink_pc;
                    let cursor_pc = dctx.cursor_pc;
                    ImGuiListClipper::new(num_lines as usize).build(|start, end| {
                        v.disasm_block(
                            (pc_range.0 + start as u64 * 4, pc_range.0 + end as u64 * 4),
                            |pc, mem, insn| {
                                let mut bkg_color = color(0, 0, 0);

                                // Highlight this line if it's the current cursor position
                                if let Some(cpc) = cursor_pc {
                                    if cpc == pc {
                                        let wsize = ui.content_region_avail();
                                        let dl = ui.get_window_draw_list();
                                        let pos = ui.cursor_screen_pos();
                                        let end = [pos[0] + wsize[0], pos[1] + 15.0];
                                        let c1 = color(151, 39, 77);
                                        dl.add_rect_filled_multicolor(pos, end, c1, c1, c1, c1);
                                        bkg_color = c1;
                                    }
                                }

                                // Highlight this line if it is PC.
                                if pc == cur_pc {
                                    let wsize = ui.content_region_avail();
                                    let dl = ui.get_window_draw_list();
                                    let pos = ui.cursor_screen_pos();
                                    let end = [pos[0] + wsize[0], pos[1] + 15.0];
                                    let c1 = color(41, 65, 100);
                                    dl.add_rect_filled_multicolor(pos, end, c1, c1, c1, c1);
                                    bkg_color = c1;

                                    // If PC changed since last time, update also the context to save
                                    // input/output regs (that will be used to highlight them).
                                    if dctx.cur_pc.is_none() || dctx.cur_pc.unwrap() != pc {
                                        dctx.cur_pc = Some(pc);

                                        dctx.regs_highlight.clear();
                                        for op in insn.args() {
                                            if let Some(inp) = op.input() {
                                                dctx.regs_highlight
                                                    .insert(inp, RegHighlight::Input);
                                            }
                                            if let Some(outp) = op.output() {
                                                dctx.regs_highlight
                                                    .insert(outp, RegHighlight::Output);
                                            }
                                        }
                                    }
                                }

                                // See if we need to do a blink animation over this PC
                                if let Some((bpc, bwhen)) = blink_pc {
                                    if v.pc_mask(bpc) == pc {
                                        match blink_color(bkg_color, bwhen) {
                                            Some(c1) => {
                                                let wsize = ui.content_region_avail();
                                                let dl = ui.get_window_draw_list();
                                                let pos = ui.cursor_screen_pos();
                                                let end = [pos[0] + wsize[0], pos[1] + 15.0];
                                                dl.add_rect_filled_multicolor(
                                                    pos, end, c1, c1, c1, c1,
                                                )
                                            }
                                            None => {}
                                        }
                                    }
                                }

                                let dis = insn.disasm();
                                let fields: Vec<&str> = dis.splitn(2, "\t").collect();

                                let gr = ui.begin_group();

                                // Address
                                ui.text_colored(color(174, 129, 255), im_str!("{:08x}", pc));

                                // Hex dump
                                ui.same_line(80.0);
                                ui.text_colored(color(102, 99, 83), im_str!("{:x}", ByteBuf(mem)));

                                // Opcode
                                ui.same_line(160.0);
                                ui.text_colored(color(165, 224, 46), im_str!("{}", fields[0]));

                                // Args
                                ui.same_line(230.0);
                                ui.text_colored(color(230, 219, 116), im_str!("{:80}", fields[1]));

                                gr.end(&ui);
                                if ui.is_item_hovered()
                                    && ui.is_window_focused()
                                    && ui.is_mouse_clicked(MouseButton::Left)
                                {
                                    dctx.cursor_pc = Some(pc);
                                }
                            },
                        );
                    })
                })
        });

    dctx.force_pc = None;

    // See if we need to set a UiCommand into the context.
    if set_command.is_some() {
        ctx.command = set_command;
    }
}
