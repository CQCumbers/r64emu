extern crate num;

use self::num::Float;
use super::cpu::{Cop, CpuContext};
use super::decode::{DecodedInsn, REG_NAMES};
use emu::dbg::{Operand, Result, Tracer};
use emu::int::Numerics;
use slog;
use slog::*;
use std::marker::PhantomData;

const FPU_CREG_NAMES: [&'static str; 32] = [
    "?0?", "?1?", "?2?", "?3?", "?4?", "?5?", "?6?", "?7?", "?8?", "?9?", "?10?", "?11?", "?12?",
    "?13?", "?14?", "?15?", "?16?", "?17?", "?18?", "?19?", "?20?", "?21?", "?22?", "?23?", "?24?",
    "?25?", "?26?", "?27?", "?28?", "?29?", "?30?", "FCSR",
];

pub struct Fpu {
    regs: [u64; 32],
    _fir: u64,
    fccr: u64,
    _fexr: u64,
    _fenr: u64,
    fcsr: u64,

    logger: slog::Logger,
    name: &'static str,
}

trait FloatRawConvert {
    fn from_u64bits(v: u64) -> Self;
    fn to_u64bits(self) -> u64;
    fn bankers_round(self) -> Self;
}

impl FloatRawConvert for f32 {
    fn from_u64bits(v: u64) -> Self {
        f32::from_bits(v as u32)
    }
    fn to_u64bits(self) -> u64 {
        self.to_bits() as u64
    }
    fn bankers_round(self) -> Self {
        let y = self.round();
        if (self - y).abs() == 0.5 {
            (self * 0.5).round() * 2.0
        } else {
            y
        }
    }
}

impl FloatRawConvert for f64 {
    fn from_u64bits(v: u64) -> Self {
        f64::from_bits(v)
    }
    fn to_u64bits(self) -> u64 {
        self.to_bits()
    }
    fn bankers_round(self) -> Self {
        let y = self.round();
        if (self - y).abs() == 0.5 {
            (self * 0.5).round() * 2.0
        } else {
            y
        }
    }
}

struct Fop<'a, F: Float + FloatRawConvert> {
    opcode: u32,
    fpu: &'a mut Fpu,
    _cpu: &'a mut CpuContext,
    phantom: PhantomData<F>,
}

impl<'a, F: Float + FloatRawConvert> Fop<'a, F> {
    fn func(&self) -> u32 {
        self.opcode & 0x3f
    }
    fn cc(&self) -> usize {
        ((self.opcode >> 8) & 7) as usize
    }
    fn rs(&self) -> usize {
        ((self.opcode >> 11) & 0x1f) as usize
    }
    fn rt(&self) -> usize {
        ((self.opcode >> 16) & 0x1f) as usize
    }
    fn rd(&self) -> usize {
        ((self.opcode >> 6) & 0x1f) as usize
    }
    fn fs(&self) -> F {
        F::from_u64bits(self.fpu.regs[self.rs()])
    }
    fn ft(&self) -> F {
        F::from_u64bits(self.fpu.regs[self.rt()])
    }
    fn set_fd(&mut self, v: F) {
        self.fpu.regs[self.rd()] = v.to_u64bits();
    }
    fn mfd64(&'a mut self) -> &'a mut u64 {
        &mut self.fpu.regs[self.rd()]
    }
}

macro_rules! approx {
    ($op:ident, $round:ident, $size:ident) => {{
        match $op.fs().$round().$size() {
            Some(v) => *$op.mfd64() = v as u64,
            None => panic!("approx out of range"),
        }
    }};
}

macro_rules! cond {
    ($op:ident, $func:expr) => {{
        let fs = $op.fs();
        let ft = $op.ft();
        let nan = fs.is_nan() || ft.is_nan();
        let less = if !nan { fs < ft } else { false };
        let equal = if !nan { fs == ft } else { false };
        if nan && $func & 8 != 0 {
            panic!("signal FPU NaN in comparison");
        }

        let cond =
            (less && ($func & 4) != 0) || (equal && ($func & 2) != 0) || (nan && ($func & 1) != 0);
        let cc = $op.cc();
        $op.fpu.set_cc(cc, cond);
    }};
}

impl Fpu {
    pub fn new(name: &'static str, logger: slog::Logger) -> Box<Fpu> {
        Box::new(Fpu {
            regs: [0u64; 32],
            _fir: 0,
            fccr: 0,
            _fexr: 0,
            _fenr: 0,
            fcsr: 0,
            logger,
            name,
        })
    }

    fn set_cc(&mut self, cc: usize, val: bool) {
        if cc > 8 {
            panic!("invalid cc code");
        }
        self.fccr = (self.fccr & !(1 << cc)) | ((val as u64) << cc);
        let mut cc2 = cc + 23;
        if cc > 0 {
            cc2 += 1;
        }
        self.fcsr = (self.fcsr & !(1 << cc2)) | ((val as u64) << cc2);
    }

    fn get_cc(&mut self, cc: usize) -> bool {
        if cc > 8 {
            panic!("invalid cc code");
        }
        (self.fccr & (1 << cc)) != 0
    }

    fn fop<M: Float + FloatRawConvert>(
        &mut self,
        cpu: &mut CpuContext,
        opcode: u32,
        _t: &Tracer,
    ) -> Result<()> {
        let mut op = Fop::<M> {
            opcode,
            fpu: self,
            _cpu: cpu,
            phantom: PhantomData,
        };
        match op.func() {
            0x00 => {
                // ADD.fmt
                let v = op.fs() + op.ft();
                op.set_fd(v)
            }
            0x01 => {
                // SUB.fmt
                let v = op.fs() - op.ft();
                op.set_fd(v)
            }
            0x02 => {
                // MUL.fmt
                let v = op.fs() * op.ft();
                op.set_fd(v)
            }
            0x03 => {
                // DIV.fmt
                let v = op.fs() / op.ft();
                op.set_fd(v)
            }
            0x04 => {
                // SQRT.fmt
                let v = op.fs().sqrt();
                op.set_fd(v)
            }
            0x05 => {
                // ABS.fmt
                let v = op.fs().abs();
                op.set_fd(v)
            }
            0x06 => {
                // MOV.fmt
                let v = op.fs();
                op.set_fd(v);
            }
            0x07 => {
                // NEG.fmt
                let v = op.fs().neg();
                op.set_fd(v)
            }
            0x08 => approx!(op, bankers_round, to_i64), // ROUND.L.fmt
            0x09 => approx!(op, trunc, to_i64),         // TRUNC.L.fmt
            0x0A => approx!(op, ceil, to_i64),          // CEIL.L.fmt
            0x0B => approx!(op, floor, to_i64),         // FLOOR.L.fmt
            0x0C => approx!(op, bankers_round, to_i32), // ROUND.W.fmt
            0x0D => approx!(op, trunc, to_i32),         // TRUNC.W.fmt
            0x0E => approx!(op, ceil, to_i32),          // CEIL.W.fmt
            0x0F => approx!(op, floor, to_i32),         // FLOOR.W.fmt

            0x30 => cond!(op, 0x30),
            0x31 => cond!(op, 0x31),
            0x32 => cond!(op, 0x32),
            0x33 => cond!(op, 0x33),
            0x34 => cond!(op, 0x34),
            0x35 => cond!(op, 0x35),
            0x36 => cond!(op, 0x36),
            0x37 => cond!(op, 0x37),
            0x38 => cond!(op, 0x38),
            0x39 => cond!(op, 0x39),
            0x3A => cond!(op, 0x3A),
            0x3B => cond!(op, 0x3B),
            0x3C => cond!(op, 0x3C),
            0x3D => cond!(op, 0x3D),
            0x3E => cond!(op, 0x3E),
            0x3F => cond!(op, 0x3F),

            _ => panic!("unimplemented COP1 opcode: func={:x?}", op.func()),
        }
        Ok(())
    }
}

impl Cop for Fpu {
    fn reg(&self, idx: usize) -> u128 {
        self.regs[idx] as u128
    }
    fn set_reg(&mut self, idx: usize, val: u128) {
        self.regs[idx] = val as u64;
    }

    fn op(&mut self, cpu: &mut CpuContext, opcode: u32, t: &Tracer) -> Result<()> {
        let fmt = (opcode >> 21) & 0x1F;
        let rt = ((opcode >> 16) & 0x1F) as usize;
        let rs = (opcode >> 11) & 0x1F;
        match fmt {
            2 => match rs {
                31 => cpu.regs[rt] = self.fcsr,
                _ => {
                    error!(self.logger, "CFC1 from unknown register: {:x}", rs);
                    return t.break_here("CFC1 from unknown register");
                },
            }
            6 => match rs {
                31 => self.fcsr = cpu.regs[rt],
                _ => {
                    error!(self.logger, "CTC1 to unknown register: {:x}", rs);
                    return t.break_here("CTC1 to unknown register");
                },
            }
            8 => {
                let tgt = cpu.pc + (opcode & 0xffff).sx64() * 4;
                let cc = ((opcode >> 18) & 3) as usize;
                let nd = opcode & (1 << 17) != 0;
                let tf = opcode & (1 << 16) != 0;
                let cond = self.get_cc(cc) == tf;
                cpu.branch(cond, tgt, nd);
            }
            16 => return self.fop::<f32>(cpu, opcode, t),
            17 => return self.fop::<f64>(cpu, opcode, t),
            _ => {
                error!(self.logger, "unimplemented COP1 fmt: fmt={:x?}", fmt);
                return t.break_here("unimplemented COP1 opcode");
            }
        }
        Ok(())
    }

    fn decode(&self, opcode: u32, pc: u64) -> DecodedInsn {
        use self::Operand::*;
        let fmt = (opcode >> 21) & 0x1F;
        let rt = REG_NAMES[((opcode >> 16) & 0x1f) as usize].into();
        let fs = FPU_CREG_NAMES[((opcode >> 11) & 0x1f) as usize].into();
        match fmt {
            2 => DecodedInsn::new2("cfc1", OReg(rt), IReg(fs)),
            6 => DecodedInsn::new2("ctc1", IReg(rt), OReg(fs)),
            8 => {
                let tgt = pc + (opcode & 0xffff).sx64() * 4;
                let cc = ((opcode >> 18) & 3) as usize;
                let nd = opcode & (1 << 17) != 0;
                let tf = opcode & (1 << 16) != 0;
                let name = if tf {
                    if nd {
                        "BC1TL"
                    } else {
                        "BC1T"
                    }
                } else {
                    if nd {
                        "BC1FL"
                    } else {
                        "BC1F"
                    }
                };
                if cc != 0 {
                    DecodedInsn::new2(name, Imm8(cc as u8), Target(tgt))
                } else {
                    DecodedInsn::new1(name, Target(tgt))
                }
            }
            _ => DecodedInsn::new1("cop1?", Imm32(fmt)),
        }
    }
}
