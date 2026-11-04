// sfs/value.rs — Kaze runtime values.
//
// PLAN-KAZE-LANGUAGE §4: the VM's tagged union. Strings live in the run's
// linear arena (`arena.rs`); arrays and maps live in the heap side tables
// (`Heap` below), which are dropped wholesale when the run ends. Nothing here
// owns a pointer into freed memory, so there is no GC and no unsafe code.

use crate::arena::StrRef;

/// A Kaze value. 16 bytes; `Copy` so the VM stack is a plain `Vec<Value>`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    /// No value: an action that returns nothing, a `let` with no initializer.
    Unit,
    Num(f64),
    Bool(bool),
    Str(StrRef),
    /// Index into `Heap::arrays`.
    Arr(u32),
    /// Index into `Heap::maps`.
    Map(u32),
    /// Index into `Heap::vecs` — a point or an offset, `[x, y, z]`.
    Vec(u32),
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Unit => "nothing",
            Value::Num(_) => "number",
            Value::Bool(_) => "true/false",
            Value::Str(_) => "text",
            Value::Arr(_) => "list",
            Value::Map(_) => "record",
            Value::Vec(_) => "point",
        }
    }

    pub fn as_num(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            _ => None,
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Unit => false,
            Value::Bool(b) => *b,
            Value::Num(n) => *n != 0.0,
            Value::Str(s) => s.len > 0,
            Value::Arr(_) | Value::Map(_) | Value::Vec(_) => true,
        }
    }
}

/// The collection heap. `arrays` and `maps` are cleared by `free_wholesale`;
/// an index is never reused while a Value could still refer to it. The
/// supervisor clears the whole heap between ticks, when no Value survives a
/// tick boundary, so a long `when`/`every` run cannot grow without bound.
#[derive(Default)]
pub struct Heap {
    pub arrays: Vec<Vec<Value>>,
    pub maps: Vec<Vec<(Value, Value)>>,
    /// Points and offsets: three `f64`s, no pointers, freed wholesale.
    pub vecs: Vec<[f64; 3]>,
}

impl Heap {
    pub fn new() -> Self {
        Heap { arrays: Vec::new(), maps: Vec::new(), vecs: Vec::new() }
    }

    pub fn push_array(&mut self, items: Vec<Value>) -> Value {
        let idx = self.arrays.len() as u32;
        self.arrays.push(items);
        Value::Arr(idx)
    }

    pub fn push_map(&mut self, entries: Vec<(Value, Value)>) -> Value {
        let idx = self.maps.len() as u32;
        self.maps.push(entries);
        Value::Map(idx)
    }

    pub fn push_vec(&mut self, v: [f64; 3]) -> Value {
        let idx = self.vecs.len() as u32;
        self.vecs.push(v);
        Value::Vec(idx)
    }

    pub fn vec3(&self, idx: u32) -> Option<[f64; 3]> {
        self.vecs.get(idx as usize).copied()
    }

    pub fn array(&self, idx: u32) -> Option<&Vec<Value>> {
        self.arrays.get(idx as usize)
    }

    pub fn map(&self, idx: u32) -> Option<&Vec<(Value, Value)>> {
        self.maps.get(idx as usize)
    }

    /// Map lookup by key (numbers and strings only; first match wins).
    pub fn map_get(&self, idx: u32, key: Value, arena: &crate::arena::Arena) -> Option<Value> {
        let entries = self.maps.get(idx as usize)?;
        for (k, v) in entries {
            if crate::arena::values_equal(*k, key, arena) {
                return Some(*v);
            }
        }
        None
    }

    /// One operation, zero fragmentation — the whole run's collections go.
    pub fn free_wholesale(&mut self) {
        self.arrays.clear();
        self.maps.clear();
        self.vecs.clear();
    }

    pub fn used(&self) -> usize {
        self.arrays.len() + self.maps.len() + self.vecs.len()
    }
}
