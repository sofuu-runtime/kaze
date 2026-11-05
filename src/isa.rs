// sfs/isa.rs — the fixed 4-byte instruction set and the compiled module format.
//
// PLAN-KAZE-LANGUAGE §4. Every instruction is one 32-bit word
// `opcode:8 | a:8 | b:8 | c:8`; instructions that need a full 32-bit operand
// (a constant, a capability, a jump offset) are followed by one immediate word.
// The encoding is little-endian and contains no timestamps, no hashes and no
// iteration over a hash map, so compiling the same source twice produces the
// same bytes on every machine — the determinism invariant (§2) has to include
// the artefact, not just the run.

use std::collections::{HashSet, VecDeque};

use crate::caps;

/// The v0.1 opcode set. Numbers are part of the on-disk format: never reuse one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Op {
    /// wide: constant index
    Const = 0,
    /// a: local slot
    LoadLocal = 1,
    /// a: local slot (pops)
    StoreLocal = 2,
    /// a: element count (pops that many)
    NewArr = 3,
    GetIndex = 4,
    Len = 5,
    Pop = 6,
    Add = 7,
    Sub = 8,
    Mul = 9,
    Div = 10,
    Mod = 11,
    Neg = 12,
    CmpLt = 13,
    CmpLe = 14,
    CmpGt = 15,
    CmpGe = 16,
    CmpEq = 17,
    CmpNe = 18,
    Not = 19,
    /// wide: relative jump, in words, from the end of this instruction
    Jmp = 20,
    JmpIfFalse = 21,
    /// a: argument count; wide: capability index
    CallCap = 22,
    /// pop the verdict value, end the item (filter allow/reject)
    PolicyStop = 23,
    /// pop the value, end the item
    Ret = 24,
    RetUnit = 25,
    /// fuel checkpoint + host-deadline poll; lives at every loop head
    FuelTick = 26,
    /// wide: constant index of the message
    Trap = 27,
    Halt = 28,
    /// wide: relative jump when the popped value is true (loops)
    JmpIfTrue = 29,
    /// pop field name, pop map, push the field (host-provided records)
    GetField = 30,
    /// wide: pop two, jump when equal (fused compare-and-branch)
    Jeq = 31,
    /// wide: pop two, jump when not equal
    Jne = 32,
    // ── v0.3: kinematics and spatial maths ──────────────────────────
    /// a: component count (3), pops that many numbers, pushes a point
    VecNew = 33,
    /// pop two points, push the great-circle distance in metres
    VecDist = 34,
    /// pop a point, push the compass bearing of the vector in degrees
    VecBearing = 35,
    /// pop two points, push the initial bearing from the first to the second
    VecBearingBetween = 36,
    /// pop a number (radians), push its sine
    Sin = 37,
    /// pop a number (radians), push its cosine
    Cos = 38,
    /// pop x then y, push atan2(y, x) in radians
    Atan2 = 39,
    /// pop a number, push its square root (a negative argument is an error)
    Sqrt = 40,
    /// pop degrees, push radians
    DegToRad = 41,
    /// pop radians, push degrees
    RadToDeg = 42,
    /// pop two points, push the great-circle distance plus altitude change
    VecSlantDist = 43,
    // ── v0.4: fault tolerance ───────────────────────────────────────
    /// a: local slot for the error record (255 = no binding);
    /// wide: relative jump to the recovery block, from the end of the instruction
    TryBegin = 44,
    /// pop the innermost active try handler
    TryEnd = 45,
    /// wide: milliseconds of link silence; push true once the link has been
    /// silent at least that long (false when there is no live link)
    LinkLost = 46,
}

impl Op {
    pub fn from_u8(b: u8) -> Option<Op> {
        Some(match b {
            0 => Op::Const,
            1 => Op::LoadLocal,
            2 => Op::StoreLocal,
            3 => Op::NewArr,
            4 => Op::GetIndex,
            5 => Op::Len,
            6 => Op::Pop,
            7 => Op::Add,
            8 => Op::Sub,
            9 => Op::Mul,
            10 => Op::Div,
            11 => Op::Mod,
            12 => Op::Neg,
            13 => Op::CmpLt,
            14 => Op::CmpLe,
            15 => Op::CmpGt,
            16 => Op::CmpGe,
            17 => Op::CmpEq,
            18 => Op::CmpNe,
            19 => Op::Not,
            20 => Op::Jmp,
            21 => Op::JmpIfFalse,
            22 => Op::CallCap,
            23 => Op::PolicyStop,
            24 => Op::Ret,
            25 => Op::RetUnit,
            26 => Op::FuelTick,
            27 => Op::Trap,
            28 => Op::Halt,
            29 => Op::JmpIfTrue,
            30 => Op::GetField,
            31 => Op::Jeq,
            32 => Op::Jne,
            33 => Op::VecNew,
            34 => Op::VecDist,
            35 => Op::VecBearing,
            36 => Op::VecBearingBetween,
            37 => Op::Sin,
            38 => Op::Cos,
            39 => Op::Atan2,
            40 => Op::Sqrt,
            41 => Op::DegToRad,
            42 => Op::RadToDeg,
            43 => Op::VecSlantDist,
            44 => Op::TryBegin,
            45 => Op::TryEnd,
            46 => Op::LinkLost,
            _ => return None,
        })
    }

    pub fn has_immediate(self) -> bool {
        matches!(
            self,
            Op::Const
                | Op::Jmp
                | Op::JmpIfFalse
                | Op::JmpIfTrue
                | Op::Jeq
                | Op::Jne
                | Op::CallCap
                | Op::Trap
                // FuelTick carries its loop's source line. It was missing here,
                // which made `compile --disasm` read that line as an opcode.
                | Op::FuelTick
                | Op::TryBegin
                | Op::LinkLost
        )
    }

    pub fn name(self) -> &'static str {
        match self {
            Op::Const => "CONST",
            Op::LoadLocal => "LOAD",
            Op::StoreLocal => "STORE",
            Op::NewArr => "NEWARR",
            Op::GetIndex => "GETINDEX",
            Op::Len => "LEN",
            Op::Pop => "POP",
            Op::Add => "ADD",
            Op::Sub => "SUB",
            Op::Mul => "MUL",
            Op::Div => "DIV",
            Op::Mod => "MOD",
            Op::Neg => "NEG",
            Op::CmpLt => "CMP_LT",
            Op::CmpLe => "CMP_LE",
            Op::CmpGt => "CMP_GT",
            Op::CmpGe => "CMP_GE",
            Op::CmpEq => "CMP_EQ",
            Op::CmpNe => "CMP_NE",
            Op::Not => "NOT",
            Op::Jmp => "JMP",
            Op::JmpIfFalse => "JMP_IF_FALSE",
            Op::CallCap => "CALL_CAP",
            Op::PolicyStop => "POLICY_STOP",
            Op::Ret => "RET",
            Op::RetUnit => "RET_NOTHING",
            Op::FuelTick => "FUEL_TICK",
            Op::Trap => "TRAP_ERR",
            Op::Halt => "HALT",
            Op::JmpIfTrue => "JMP_IF_TRUE",
            Op::GetField => "GET_FIELD",
            Op::Jeq => "JEQ",
            Op::Jne => "JNE",
            Op::VecNew => "VECNEW",
            Op::VecDist => "VECDIST",
            Op::VecBearing => "VECBEARING",
            Op::VecBearingBetween => "VECBEARING_BETWEEN",
            Op::Sin => "SIN",
            Op::Cos => "COS",
            Op::Atan2 => "ATAN2",
            Op::Sqrt => "SQRT",
            Op::DegToRad => "DEG_TO_RAD",
            Op::RadToDeg => "RAD_TO_DEG",
            Op::VecSlantDist => "VECSLANT_DIST",
            Op::TryBegin => "TRY_BEGIN",
            Op::TryEnd => "TRY_END",
            Op::LinkLost => "LINK_LOST",
        }
    }
}

/// Encode `op a b c` as one word.
pub fn word(op: Op, a: u8, b: u8, c: u8) -> u32 {
    (op as u32) | ((a as u32) << 8) | ((b as u32) << 16) | ((c as u32) << 24)
}

pub fn opcode(w: u32) -> u8 {
    (w & 0xff) as u8
}

pub fn field_a(w: u32) -> u8 {
    ((w >> 8) & 0xff) as u8
}

#[derive(Clone, Debug, PartialEq)]
pub enum Const {
    Num(f64),
    Str(String),
    Bool(bool),
}

/// One capability call site: what the VM has to know before it dispatches.
#[derive(Clone, Debug, PartialEq)]
pub struct CapRef {
    pub family: String,
    pub op: String,
    pub argc: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemTag {
    Action = 0,
    Check = 1,
    Filter = 2,
    When = 3,
    Every = 4,
}

impl ItemTag {
    pub fn from_u8(b: u8) -> Option<ItemTag> {
        Some(match b {
            0 => ItemTag::Action,
            1 => ItemTag::Check,
            2 => ItemTag::Filter,
            3 => ItemTag::When,
            4 => ItemTag::Every,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            ItemTag::Action => "action",
            ItemTag::Check => "check",
            ItemTag::Filter => "filter",
            ItemTag::When => "when",
            ItemTag::Every => "every",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ItemCode {
    pub name: String,
    pub tag: ItemTag,
    pub params: Vec<String>,
    pub code: Vec<u32>,
    pub line: u32,
    /// The file this item came from. The compiler copies it for every item;
    /// the VM uses it to attribute a run-time error. Empty only for a module
    /// built without the loader.
    pub origin: String,
    /// Period in milliseconds; 0 for everything but `every`.
    pub period_ms: f64,
    /// Number of local slots the item uses (parameters occupy the first ones).
    pub slots: u8,
}

#[derive(Clone, Debug, Default)]
pub struct Module {
    pub needs: Vec<String>,
    /// The root script's diagnostic origin (its file name). Items whose
    /// `origin` equals this are the root file's own; the rest were imported.
    pub root: String,
    pub consts: Vec<Const>,
    pub caps: Vec<CapRef>,
    pub items: Vec<ItemCode>,
}

impl Module {
    pub fn item(&self, name: &str) -> Option<&ItemCode> {
        self.items.iter().find(|i| i.name == name)
    }

    /// Byte size of the executable part, for the CLI's "compiled" line.
    pub fn code_bytes(&self) -> usize {
        self.items.iter().map(|i| i.code.len() * 4).sum()
    }
}

const MAGIC: &[u8; 8] = b"KAZEBC1\0";
/// 4 since v0.5 added the root name and the per-item source origin (for
/// multi-file diagnostics and root-only entry resolution). Bumping the version
/// makes an older reader refuse the module up front instead of misreading the
/// new fields. Format 2 (v0.3) and 3 (v0.4) modules must be recompiled.
const VERSION: u16 = 4;

/// The on-disk module format, exposed for the compatibility lock.
pub const FORMAT_VERSION: u16 = VERSION;

/// The operand stack bound the VM enforces; the verifier proves it statically.
pub const MAX_STACK: usize = 256;

struct Writer {
    out: Vec<u8>,
}

impl Writer {
    fn u8(&mut self, v: u8) {
        self.out.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }
    fn f64(&mut self, v: f64) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }
    fn text(&mut self, s: &str) {
        let b = s.as_bytes();
        self.u32(b.len() as u32);
        self.out.extend_from_slice(b);
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.buf.get(self.at)?;
        self.at += 1;
        Some(v)
    }
    fn u16(&mut self) -> Option<u16> {
        let b = self.buf.get(self.at..self.at + 2)?;
        self.at += 2;
        Some(u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        let b = self.buf.get(self.at..self.at + 4)?;
        self.at += 4;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn f64(&mut self) -> Option<f64> {
        let b = self.buf.get(self.at..self.at + 8)?;
        self.at += 8;
        Some(f64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
    fn text(&mut self) -> Option<String> {
        let len = self.u32()? as usize;
        let b = self.buf.get(self.at..self.at + len)?;
        self.at += len;
        String::from_utf8(b.to_vec()).ok()
    }
}

/// Serialize a module. Deterministic: same module, same bytes.
pub fn serialize(m: &Module) -> Vec<u8> {
    let mut w = Writer { out: Vec::with_capacity(256) };
    w.out.extend_from_slice(MAGIC);
    w.u16(VERSION);
    w.text(&m.root);
    w.u32(m.needs.len() as u32);
    for n in &m.needs {
        w.text(n);
    }
    w.u32(m.consts.len() as u32);
    for c in &m.consts {
        match c {
            Const::Num(n) => {
                w.u8(0);
                w.f64(*n);
            }
            Const::Str(s) => {
                w.u8(1);
                w.text(s);
            }
            Const::Bool(b) => {
                w.u8(2);
                w.u8(if *b { 1 } else { 0 });
            }
        }
    }
    w.u32(m.caps.len() as u32);
    for c in &m.caps {
        w.text(&c.family);
        w.text(&c.op);
        w.u8(c.argc);
    }
    w.u32(m.items.len() as u32);
    for it in &m.items {
        w.u8(it.tag as u8);
        w.f64(it.period_ms);
        w.u32(it.line);
        w.u8(it.slots);
        w.text(&it.name);
        w.text(&it.origin);
        w.u32(it.params.len() as u32);
        for p in &it.params {
            w.text(p);
        }
        w.u32(it.code.len() as u32);
        for word in &it.code {
            w.u32(*word);
        }
    }
    w.out
}

pub fn deserialize(bytes: &[u8]) -> Result<Module, String> {
    let mut r = Reader { buf: bytes, at: 0 };
    let magic = bytes.get(0..8).ok_or("not a Kaze module: too short")?;
    if magic != MAGIC {
        return Err("not a Kaze module: the magic number does not match".to_string());
    }
    r.at = 8;
    match r.u16() {
        Some(VERSION) => {}
        Some(other) => return Err(format!("this module was built by Kaze version {}, not {}", other, VERSION)),
        None => return Err("truncated module header".to_string()),
    }
    let root = r.text().ok_or("truncated module root")?;
    let mut m = Module { root, ..Default::default() };
    let needs = r.u32().ok_or("truncated needs table")?;
    for _ in 0..needs {
        m.needs.push(r.text().ok_or("truncated needs entry")?);
    }
    let consts = r.u32().ok_or("truncated constant table")?;
    for _ in 0..consts {
        match r.u8().ok_or("truncated constant")? {
            0 => m.consts.push(Const::Num(r.f64().ok_or("truncated number constant")?)),
            1 => m.consts.push(Const::Str(r.text().ok_or("truncated text constant")?)),
            2 => m.consts.push(Const::Bool(r.u8().ok_or("truncated boolean constant")? != 0)),
            other => return Err(format!("unknown constant kind {}", other)),
        }
    }
    let caps = r.u32().ok_or("truncated capability table")?;
    for _ in 0..caps {
        let family = r.text().ok_or("truncated capability")?;
        let op = r.text().ok_or("truncated capability")?;
        let argc = r.u8().ok_or("truncated capability")?;
        m.caps.push(CapRef { family, op, argc });
    }
    let items = r.u32().ok_or("truncated item table")?;
    for _ in 0..items {
        let tag = ItemTag::from_u8(r.u8().ok_or("truncated item")?).ok_or("unknown item kind")?;
        let period_ms = r.f64().ok_or("truncated item period")?;
        let line = r.u32().ok_or("truncated item line")?;
        let slots = r.u8().ok_or("truncated item slots")?;
        let name = r.text().ok_or("truncated item name")?;
        let origin = r.text().ok_or("truncated item origin")?;
        let nparams = r.u32().ok_or("truncated item parameters")?;
        let mut params = Vec::new();
        for _ in 0..nparams {
            params.push(r.text().ok_or("truncated parameter name")?);
        }
        let ncode = r.u32().ok_or("truncated code")? as usize;
        let mut code = Vec::with_capacity(ncode.min(1 << 20));
        for _ in 0..ncode {
            code.push(r.u32().ok_or("truncated code word")?);
        }
        m.items.push(ItemCode { name, tag, params, code, line, origin, period_ms, slots });
    }
    verify(&m)?;
    Ok(m)
}

struct Insn {
    at: usize,
    op: Op,
    a: u8,
    imm: Option<u32>,
    next: usize,
}

/// Verify a module before it is executed.
///
/// Structural checks prove every opcode and immediate is present, every table
/// index is in range, every jump lands on an instruction boundary, and every
/// local slot exists. A stack-depth dataflow then proves that no reachable
/// instruction underflows the operand stack, that branches merge at the same
/// depth, and that the depth never exceeds `MAX_STACK`. A module that passes
/// cannot make the VM index outside its stack; a module that fails is refused
/// before the first instruction runs.
pub fn verify(module: &Module) -> Result<(), String> {
    for item in &module.items {
        verify_item(module, item)?;
    }
    Ok(())
}

fn verify_item(module: &Module, item: &ItemCode) -> Result<(), String> {
    if item.params.len() > 255 {
        return Err(format!("{}: more than 255 parameters", item.name));
    }
    let locals = item.slots.max(item.params.len() as u8) as usize;
    let code = &item.code;
    let mut insns: Vec<Insn> = Vec::new();
    let mut index: Vec<Option<usize>> = vec![None; code.len() + 1];
    let mut boundaries: HashSet<usize> = HashSet::new();
    let mut at = 0usize;
    while at < code.len() {
        boundaries.insert(at);
        let w = code[at];
        let op = Op::from_u8(opcode(w))
            .ok_or_else(|| format!("{}: unknown opcode {} at {}", item.name, opcode(w), at))?;
        let imm = if op.has_immediate() {
            Some(*code.get(at + 1).ok_or_else(|| {
                format!("{}: {} at {} has no immediate", item.name, op.name(), at)
            })?)
        } else {
            None
        };
        let next = at + 1 + usize::from(op.has_immediate());
        index[at] = Some(insns.len());
        insns.push(Insn { at, op, a: field_a(w), imm, next });
        at = next;
    }
    boundaries.insert(code.len());

    for insn in &insns {
        verify_indices(module, item, insn, locals, &boundaries)?;
    }
    verify_stack(item, &insns, &index, code.len())
}

fn verify_indices(
    module: &Module,
    item: &ItemCode,
    insn: &Insn,
    locals: usize,
    boundaries: &HashSet<usize>,
) -> Result<(), String> {
    let here = || format!("{} at instruction {}", item.name, insn.at);
    match insn.op {
        Op::LoadLocal | Op::StoreLocal => {
            if insn.a as usize >= locals {
                return Err(format!("{}: local slot {} is out of range", here(), insn.a));
            }
        }
        Op::TryBegin => {
            if insn.a != 255 && insn.a as usize >= locals {
                return Err(format!("{}: error slot {} is out of range", here(), insn.a));
            }
            verify_jump(item, insn, boundaries)?;
        }
        Op::Const | Op::Trap => {
            let imm = insn.imm.unwrap_or(u32::MAX) as usize;
            if imm >= module.consts.len() {
                return Err(format!("{}: constant {} is out of range", here(), imm));
            }
        }
        Op::CallCap => {
            let imm = insn.imm.unwrap_or(u32::MAX) as usize;
            let cap = module
                .caps
                .get(imm)
                .ok_or_else(|| format!("{}: capability {} is out of range", here(), imm))?;
            if insn.a as usize != cap.argc as usize {
                return Err(format!(
                    "{}: CALL_CAP says {} arguments but cap[{}] says {}",
                    here(),
                    insn.a,
                    imm,
                    cap.argc
                ));
            }
            if !caps::known_op(&cap.family, &cap.op) {
                return Err(format!(
                    "{}: {}.{} is not a registered capability",
                    here(),
                    cap.family,
                    cap.op
                ));
            }
            // The artifact's own `argc` is attacker-controlled; the registry
            // is the authority. `drone.goto` with zero arguments passes the
            // H-4 gate (a missing argument is not ranged) and would fly to
            // telemetry defaults instead of being refused.
            if let Some((min, max)) = caps::op_arity(&cap.family, &cap.op) {
                let argc = insn.a as usize;
                if argc < min || argc > max {
                    return Err(format!(
                        "{}: {}.{} takes {} to {} value(s), not {}",
                        here(),
                        cap.family,
                        cap.op,
                        min,
                        max,
                        argc
                    ));
                }
            }
        }
        Op::VecNew => {
            if insn.a != 2 && insn.a != 3 {
                return Err(format!("{}: VECNEW width {} is not 2 or 3", here(), insn.a));
            }
        }
        Op::Jmp | Op::JmpIfFalse | Op::JmpIfTrue | Op::Jeq | Op::Jne => {
            verify_jump(item, insn, boundaries)?;
        }
        _ => {}
    }
    Ok(())
}

fn verify_jump(
    item: &ItemCode,
    insn: &Insn,
    boundaries: &HashSet<usize>,
) -> Result<(), String> {
    let target = jump_target(insn);
    if target < 0 || target as usize > item.code.len() {
        return Err(format!(
            "{} at instruction {}: jump target {} is outside the item",
            item.name, insn.at, target
        ));
    }
    if (target as usize) < item.code.len() && !boundaries.contains(&(target as usize)) {
        return Err(format!(
            "{} at instruction {}: jump target {} is not an instruction boundary",
            item.name, insn.at, target
        ));
    }
    Ok(())
}

fn jump_target(insn: &Insn) -> i64 {
    insn.next as i64 + insn.imm.unwrap_or(0) as i32 as i64
}

fn stack_effect(op: Op, a: u8) -> Option<(usize, usize)> {
    let a = a as usize;
    Some(match op {
        Op::Const | Op::LoadLocal | Op::LinkLost => (0, 1),
        Op::StoreLocal | Op::Pop | Op::JmpIfFalse | Op::JmpIfTrue | Op::PolicyStop | Op::Ret => {
            (1, 0)
        }
        // Fused compare-and-branch: pop two, push nothing (unlike CMP_*).
        Op::Jeq | Op::Jne => (2, 0),
        Op::NewArr | Op::VecNew => (a, 1),
        Op::GetIndex
        | Op::GetField
        | Op::Add
        | Op::Sub
        | Op::Mul
        | Op::Div
        | Op::Mod
        | Op::CmpLt
        | Op::CmpLe
        | Op::CmpGt
        | Op::CmpGe
        | Op::CmpEq
        | Op::CmpNe
        | Op::VecDist
        | Op::VecSlantDist
        | Op::VecBearingBetween
        | Op::Atan2 => (2, 1),
        Op::Len
        | Op::Neg
        | Op::Not
        | Op::VecBearing
        | Op::Sin
        | Op::Cos
        | Op::Sqrt
        | Op::DegToRad
        | Op::RadToDeg => (1, 1),
        Op::CallCap => (a, 1),
        Op::Jmp | Op::FuelTick | Op::TryBegin | Op::TryEnd | Op::RetUnit | Op::Halt | Op::Trap => {
            (0, 0)
        }
    })
}

fn successors(insn: &Insn) -> Vec<usize> {
    match insn.op {
        Op::Ret | Op::RetUnit | Op::PolicyStop | Op::Halt | Op::Trap => Vec::new(),
        Op::Jmp => vec![jump_target(insn) as usize],
        Op::JmpIfFalse | Op::JmpIfTrue | Op::Jeq | Op::Jne | Op::TryBegin => {
            vec![jump_target(insn) as usize, insn.next]
        }
        _ => vec![insn.next],
    }
}

fn verify_stack(
    item: &ItemCode,
    insns: &[Insn],
    index: &[Option<usize>],
    len: usize,
) -> Result<(), String> {
    if insns.is_empty() {
        return Ok(());
    }
    let mut depth: Vec<Option<usize>> = vec![None; len + 1];
    let mut queue: VecDeque<usize> = VecDeque::new();
    depth[0] = Some(0);
    queue.push_back(0);
    while let Some(pc) = queue.pop_front() {
        let Some(slot) = index.get(pc).copied().flatten() else {
            return Err(format!(
                "{}: execution reaches {}, which is not an instruction",
                item.name, pc
            ));
        };
        let insn = &insns[slot];
        let d = depth[pc].unwrap_or(0);
        // A handler cannot restore a stack the VM has already popped below
        // `TRY_BEGIN` (truncate cannot grow). The compiler emits TRY_BEGIN at a
        // statement boundary, so depth 0 is the only sound placement.
        if insn.op == Op::TryBegin && d != 0 {
            return Err(format!(
                "{}: TRY_BEGIN at stack depth {} (must be 0; the handler cannot restore a deeper stack)",
                item.name, d
            ));
        }
        let (pop, push) = stack_effect(insn.op, insn.a)
            .ok_or_else(|| format!("{}: no stack effect for {}", item.name, insn.op.name()))?;
        if d < pop {
            return Err(format!(
                "{}: stack underflow at {} ({} needs {}, has {})",
                item.name,
                pc,
                insn.op.name(),
                pop,
                d
            ));
        }
        let out = d - pop + push;
        if out > MAX_STACK {
            return Err(format!("{}: stack depth {} exceeds {}", item.name, out, MAX_STACK));
        }
        for succ in successors(insn) {
            if succ > len {
                return Err(format!("{}: successor {} is outside the item", item.name, succ));
            }
            if succ == len {
                continue;
            }
            match depth[succ] {
                None => {
                    depth[succ] = Some(out);
                    queue.push_back(succ);
                }
                Some(existing) if existing != out => {
                    return Err(format!(
                        "{}: inconsistent stack depth at {} ({} and {})",
                        item.name, succ, existing, out
                    ));
                }
                Some(_) => {}
            }
        }
    }
    Ok(())
}

/// Human-readable listing, for `sofuu script compile --disasm`.
pub fn disassemble(m: &Module) -> String {
    let mut out = String::new();
    if !m.root.is_empty() {
        out.push_str(&format!("root: {}\n", m.root));
    }
    out.push_str(&format!("needs: {}\n", m.needs.join(", ")));
    for (i, c) in m.caps.iter().enumerate() {
        out.push_str(&format!("cap[{}] = {}.{}/{}\n", i, c.family, c.op, c.argc));
    }
    for (i, c) in m.consts.iter().enumerate() {
        match c {
            Const::Num(n) => out.push_str(&format!("const[{}] = {}\n", i, crate::arena::fmt_num(*n))),
            Const::Str(s) => out.push_str(&format!("const[{}] = {:?}\n", i, s)),
            Const::Bool(b) => out.push_str(&format!("const[{}] = {}\n", i, b)),
        }
    }
    for it in &m.items {
        let origin = if it.origin.is_empty() { "" } else { it.origin.as_str() };
        out.push_str(&format!(
            "\n{} {} ({} slots) [{}]\n",
            it.tag.label(),
            it.name,
            it.slots,
            origin
        ));
        let mut i = 0usize;
        while i < it.code.len() {
            let w = it.code[i];
            let Some(op) = Op::from_u8(opcode(w)) else {
                out.push_str(&format!("  {:04}  <unknown op {}>\n", i, opcode(w)));
                i += 1;
                continue;
            };
            if op.has_immediate() {
                let imm = it.code.get(i + 1).copied().unwrap_or(0);
                out.push_str(&format!(
                    "  {:04}  {} {} {} {}  (imm {})\n",
                    i,
                    op.name(),
                    field_a(w),
                    (w >> 16) & 0xff,
                    (w >> 24) & 0xff,
                    imm
                ));
                i += 2;
            } else {
                out.push_str(&format!("  {:04}  {}\n", i, op.name()));
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_module() {
        let m = Module {
            needs: vec!["drone".into()],
            root: "patrol.kaze".into(),
            consts: vec![Const::Num(15.0), Const::Str("hello".into())],
            caps: vec![CapRef { family: "drone".into(), op: "takeoff".into(), argc: 1 }],
            items: vec![ItemCode {
                name: "patrol".into(),
                tag: ItemTag::Action,
                params: vec!["points".into()],
                code: vec![word(Op::Const, 0, 0, 0), 0, word(Op::Halt, 0, 0, 0)],
                line: 3,
                origin: "patrol.kaze".to_string(),
                period_ms: 0.0,
                slots: 2,
            }],
        };
        let bytes = serialize(&m);
        let back = deserialize(&bytes).unwrap();
        assert_eq!(back.needs, m.needs);
        assert_eq!(back.root, "patrol.kaze");
        assert_eq!(back.items[0].code, m.items[0].code);
        assert_eq!(back.items[0].origin, "patrol.kaze");
        assert_eq!(back.consts, m.consts);
    }

    #[test]
    fn a_truncated_item_origin_is_refused() {
        let m = Module {
            items: vec![ItemCode {
                name: "a".into(),
                tag: ItemTag::Action,
                params: Vec::new(),
                code: vec![word(Op::RetUnit, 0, 0, 0)],
                line: 1,
                origin: "lib/x.kaze".to_string(),
                period_ms: 0.0,
                slots: 0,
            }],
            ..Default::default()
        };
        let mut bytes = serialize(&m);
        let needle = b"lib/x.kaze";
        let at = bytes
            .windows(needle.len())
            .position(|w| w == &needle[..])
            .expect("the origin is in the artifact");
        bytes.truncate(at + 2);
        let err = deserialize(&bytes).expect_err("a cut inside the origin is refused");
        assert!(err.contains("truncated item origin"), "{}", err);
    }

    #[test]
    fn serialization_is_byte_identical_across_runs() {
        let m = Module {
            needs: vec!["log".into()],
            root: "a.kaze".into(),
            consts: vec![Const::Str("x".into())],
            caps: vec![],
            items: vec![ItemCode {
                name: "a".into(),
                tag: ItemTag::Action,
                params: vec![],
                code: vec![word(Op::RetUnit, 0, 0, 0)],
                line: 1,
                origin: String::new(),
                period_ms: 0.0,
                slots: 0,
            }],
        };
        assert_eq!(serialize(&m), serialize(&m));
    }

    #[test]
    fn an_older_module_format_is_refused_up_front() {
        for old in [2u16, 3u16] {
            let mut bytes = serialize(&Module::default());
            // MAGIC is 8 bytes; the format version is the next little-endian u16.
            bytes[8..10].copy_from_slice(&old.to_le_bytes());
            let err = deserialize(&bytes)
                .expect_err("a pre-v0.5 artifact is not a v0.5 artifact");
            assert!(err.contains("version"), "{}", err);
        }
    }

    #[test]
    fn garbage_is_refused() {
        assert!(deserialize(b"nope").is_err());
        let mut bytes = serialize(&Module::default());
        bytes[0] = b'X';
        assert!(deserialize(&bytes).is_err());
    }

    #[test]
    fn opcodes_round_trip() {
        for b in 0u8..=46 {
            let op = Op::from_u8(b).expect("known opcode");
            assert_eq!(op as u8, b);
        }
        assert!(Op::from_u8(200).is_none());
    }
}
