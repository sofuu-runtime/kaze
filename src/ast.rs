// sfs/ast.rs — the Kaze abstract syntax tree.
//
// PLAN-KAZE-LANGUAGE §5 (M-K1). Every node carries the line it came from so
// both the checker and the VM can point at real source, and the tree holds no
// references: parsing owns its data, so a compiled module outlives the source
// string.

/// A dimensional vector: the exponents of length, time, mass, angle,
/// temperature, pressure and electric current. `Dim::NONE` is a plain number.
///
/// Phase 3 of the roadmap, but the type lives here because units are written
/// next to literals: the lexer/parser normalise every literal into its base
/// unit, so a dimension is all that survives runtime — there is no conversion
/// check left to perform once the program is checked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dim {
    pub l: i8,
    pub t: i8,
    pub m: i8,
    pub a: i8,
    pub k: i8,
    pub p: i8,
    pub e: i8,
}

impl Dim {
    pub const NONE: Dim = Dim { l: 0, t: 0, m: 0, a: 0, k: 0, p: 0, e: 0 };
    pub const LENGTH: Dim = Dim { l: 1, ..Dim::NONE };
    pub const TIME: Dim = Dim { t: 1, ..Dim::NONE };
    pub const MASS: Dim = Dim { m: 1, ..Dim::NONE };
    pub const ANGLE: Dim = Dim { a: 1, ..Dim::NONE };
    pub const TEMPERATURE: Dim = Dim { k: 1, ..Dim::NONE };
    pub const PRESSURE: Dim = Dim { p: 1, ..Dim::NONE };
    pub const CURRENT: Dim = Dim { e: 1, ..Dim::NONE };
    /// `m/s`
    pub const VELOCITY: Dim = Dim { l: 1, t: -1, ..Dim::NONE };
    /// `m/s2`
    pub const ACCELERATION: Dim = Dim { l: 1, t: -2, ..Dim::NONE };
    /// `km/h`, which is the same dimension as `m/s`.
    pub const SPEED: Dim = Dim::VELOCITY;

    /// Sum of two dimension vectors (multiplication of quantities).
    pub fn plus(self, o: Dim) -> Dim {
        Dim {
            l: self.l + o.l,
            t: self.t + o.t,
            m: self.m + o.m,
            a: self.a + o.a,
            k: self.k + o.k,
            p: self.p + o.p,
            e: self.e + o.e,
        }
    }

    /// Difference of two dimension vectors (division of quantities).
    pub fn minus(self, o: Dim) -> Dim {
        Dim {
            l: self.l - o.l,
            t: self.t - o.t,
            m: self.m - o.m,
            a: self.a - o.a,
            k: self.k - o.k,
            p: self.p - o.p,
            e: self.e - o.e,
        }
    }

    pub fn scale(self, n: i8) -> Dim {
        Dim {
            l: self.l * n,
            t: self.t * n,
            m: self.m * n,
            a: self.a * n,
            k: self.k * n,
            p: self.p * n,
            e: self.e * n,
        }
    }

    pub fn is_none(self) -> bool {
        self == Dim::NONE
    }

    /// The physical quantity a dimension names, when it has a word.
    pub fn quantity(self) -> Option<&'static str> {
        match (self.l, self.t, self.m, self.a, self.k, self.p, self.e) {
            (0, 0, 0, 0, 0, 0, 0) => Some("plain number"),
            (1, 0, 0, 0, 0, 0, 0) => Some("length"),
            (0, 1, 0, 0, 0, 0, 0) => Some("time"),
            (0, 0, 1, 0, 0, 0, 0) => Some("mass"),
            (0, 0, 0, 1, 0, 0, 0) => Some("angle"),
            (0, 0, 0, 0, 1, 0, 0) => Some("temperature"),
            (0, 0, 0, 0, 0, 1, 0) => Some("pressure"),
            (0, 0, 0, 0, 0, 0, 1) => Some("current"),
            (1, -1, 0, 0, 0, 0, 0) => Some("velocity"),
            (1, -2, 0, 0, 0, 0, 0) => Some("acceleration"),
            _ => None,
        }
    }

    /// The base unit a dimension is stored in, for diagnostics.
    pub fn base_unit(self) -> &'static str {
        match self.quantity() {
            Some("length") => "meter",
            Some("time") => "second",
            Some("mass") => "kilogram",
            Some("angle") => "degree",
            Some("temperature") => "kelvin",
            Some("pressure") => "pascal",
            Some("current") => "ampere",
            Some("velocity") => "m/s",
            Some("acceleration") => "m/s2",
            _ => "number",
        }
    }

    /// `length (meter)` — the phrase a diagnostic uses.
    pub fn describe(self) -> String {
        if self.is_none() {
            return "a plain number".to_string();
        }
        match self.quantity() {
            Some(q) => format!("{} ({})", q, self.base_unit()),
            None => "a derived unit".to_string(),
        }
    }
}

/// A physical unit written next to a number.
///
/// Every literal is normalised into its dimension's base unit at parse time
/// (`1 km` becomes `1000` with dimension length), so the compiler never emits
/// a conversion: the arithmetic is already in one unit. What survives on the
/// node is the unit as written, which is what the checker, formatter and
/// machine-syntax emitter need to print it back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Unit {
    None,
    Meter,
    Km,
    Cm,
    Mm,
    Mps,
    Kmh,
    Mps2,
    Deg,
    Rad,
    Sec,
    Ms,
    Hour,
    Minute,
    Kg,
    Gram,
    Celsius,
    Kelvin,
    Pascal,
    Mbar,
    Hpa,
    Ampere,
    /// Whole numbers, as written: `20%` is 20, not 0.2.
    Pct,
}

impl Unit {
    /// The dimension this unit measures.
    pub fn dim(self) -> Dim {
        match self {
            Unit::None | Unit::Pct => Dim::NONE,
            Unit::Meter | Unit::Km | Unit::Cm | Unit::Mm => Dim::LENGTH,
            Unit::Mps | Unit::Kmh => Dim::VELOCITY,
            Unit::Mps2 => Dim::ACCELERATION,
            Unit::Deg | Unit::Rad => Dim::ANGLE,
            Unit::Sec | Unit::Ms | Unit::Hour | Unit::Minute => Dim::TIME,
            Unit::Kg | Unit::Gram => Dim::MASS,
            Unit::Celsius | Unit::Kelvin => Dim::TEMPERATURE,
            Unit::Pascal | Unit::Mbar | Unit::Hpa => Dim::PRESSURE,
            Unit::Ampere => Dim::CURRENT,
        }
    }

    /// Multiplier from this unit to the dimension's base unit. Applied once, at
    /// parse time, so no runtime scaling exists.
    pub fn scale(self) -> f64 {
        match self {
            Unit::None | Unit::Meter | Unit::Mps | Unit::Mps2 | Unit::Deg | Unit::Sec
            | Unit::Kg | Unit::Kelvin | Unit::Pascal | Unit::Ampere | Unit::Pct => 1.0,
            Unit::Km => 1000.0,
            Unit::Cm => 0.01,
            Unit::Mm => 0.001,
            Unit::Kmh => 1000.0 / 3600.0,
            Unit::Rad => 180.0 / std::f64::consts::PI,
            Unit::Hour => 3600.0,
            Unit::Minute => 60.0,
            Unit::Ms => 0.001,
            Unit::Gram => 0.001,
            Unit::Celsius => 1.0,
            Unit::Mbar => 100.0,
            Unit::Hpa => 100.0,
        }
    }

    /// Offset added after scaling, for units whose zero is not the dimension's
    /// zero (only Celsius today).
    pub fn offset(self) -> f64 {
        match self {
            Unit::Celsius => 273.15,
            _ => 0.0,
        }
    }

    /// The canonical spelling the formatter and machine emitter write back.
    pub fn written(self) -> &'static str {
        match self {
            Unit::None => "",
            Unit::Meter => "meters",
            Unit::Km => "km",
            Unit::Cm => "cm",
            Unit::Mm => "mm",
            Unit::Mps => "m/s",
            Unit::Kmh => "km/h",
            Unit::Mps2 => "m/s2",
            Unit::Deg => "degrees",
            Unit::Rad => "rad",
            Unit::Sec => "seconds",
            Unit::Ms => "ms",
            Unit::Hour => "hours",
            Unit::Minute => "minutes",
            Unit::Kg => "kg",
            Unit::Gram => "gram",
            Unit::Celsius => "Celsius",
            Unit::Kelvin => "Kelvin",
            Unit::Pascal => "Pascal",
            Unit::Mbar => "mbar",
            Unit::Hpa => "hPa",
            Unit::Ampere => "ampere",
            Unit::Pct => "%",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Unit::None => "a plain number",
            Unit::Pct => "%",
            other => other.written(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

impl BinOp {
    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::And => "and",
            BinOp::Or => "or",
        }
    }

    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Eq | BinOp::Ne
        )
    }
}

#[derive(Clone, Debug)]
pub enum Expr {
    Num(f64, Unit),
    Str(String),
    Bool(bool),
    /// A parameter or `let` binding.
    Name(String),
    /// A bare telemetry field (`battery`), read through the script's sole
    /// hardware capability.
    Ambient { field: String, family: &'static str },
    /// `recv.op(args)` — a capability call. `recv` is a declared resource.
    Call {
        recv: String,
        op: String,
        args: Vec<Expr>,
        line: u32,
    },
    /// `record.field` — a field read on a map value (a host-provided record).
    /// Maps have no literal syntax in v0.1; they arrive as arguments, and this
    /// is how a script reads one.
    Field { base: String, field: String, line: u32 },
    /// `func(arg1, arg2)` — a call to an in-module routine.
    CallRoutine {
        name: String,
        args: Vec<Expr>,
        line: u32,
    },
    /// `RecordName(field1: val1, ...)`
    RecordInit {
        name: String,
        fields: Vec<(String, Expr)>,
        line: u32,
    },
    Bin(BinOp, Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    /// `length of x`
    Len(Box<Expr>),
    Index(Box<Expr>, Box<Expr>),
    /// `[a, b, c]`
    List(Vec<Expr>),
    /// A pure maths form: `vector(x, y, z)`, `distance between a and b`,
    /// `bearing to v`, `sin(x)`, `<x> in radians`. No capability, no host.
    Math { kind: MathKind, args: Vec<Expr>, line: u32 },
    /// `link lost for 3 seconds` — true when the live link has been silent for
    /// at least the given duration. A read, not an effect: it never crosses the
    /// capability gate, and the checker requires `needs: drone`.
    LinkLost { after_ms: f64, line: u32 },
    /// `<fallible> otherwise <fallback>` — the inline recovery the checker
    /// requires on every operation it cannot prove safe (division, modulo, a
    /// dynamic index, a value-returning capability call).
    Fallback {
        inner: Box<Expr>,
        fallback: Box<Expr>,
        line: u32,
    },
}

/// The v0.3 maths forms. Each one is a pure function of its arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MathKind {
    /// `vector(x, y, z)` — three components.
    Vector,
    /// `distance between a and b` — metres on the WGS84 sphere.
    Distance,
    /// `bearing of v` — the compass direction of a local offset.
    BearingOf,
    /// `bearing from a to b` — the initial bearing to a place. `bearing to b`
    /// is parsed into this, with `a` read from the machine's own position.
    BearingFrom,
    /// `slant distance between a and b` — great circle plus altitude change.
    SlantDistance,
    /// `sin(x)`, `cos(x)` — radians in, plain number out.
    Sin,
    Cos,
    /// `atan2(y, x)` — radians, the same convention as `f64::atan2`.
    Atan2,
    /// `sqrt(x)` — refuses a negative argument instead of returning NaN.
    Sqrt,
    /// `<x> in radians` — degrees to radians.
    ToRadians,
    /// `<x> in degrees` — radians to degrees.
    ToDegrees,
}

impl Expr {
    pub fn line(&self) -> u32 {
        match self {
            Expr::Call { line, .. }
            | Expr::CallRoutine { line, .. }
            | Expr::RecordInit { line, .. }
            | Expr::Field { line, .. }
            | Expr::Math { line, .. }
            | Expr::LinkLost { line, .. }
            | Expr::Fallback { line, .. } => *line,
            _ => 0,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Let(String, Expr, u32),
    Set(String, Expr, u32),
    /// An expression run for its effect (a capability call).
    Eval(Expr, u32),
    If {
        cond: Expr,
        then: Vec<Stmt>,
        otherwise: Vec<Stmt>,
        line: u32,
    },
    ForEach {
        var: String,
        list: Expr,
        body: Vec<Stmt>,
        line: u32,
    },
    /// `while <cond> bounded <n>:` — the bound is mandatory (Phase 2): a loop
    /// the compiler cannot bound is a loop that can hang a vehicle. `None` only
    /// appears before the checker has run, so it can report the omission.
    While {
        cond: Expr,
        body: Vec<Stmt>,
        line: u32,
        bound: Option<u32>,
    },
    /// `try:` … `otherwise [with err]:` — recover from an operational host
    /// failure (a link, a rejected command, an arming denial). Safety, range,
    /// fuel and budget errors are never catchable.
    Try {
        body: Vec<Stmt>,
        otherwise: Vec<Stmt>,
        /// `otherwise with err:` binds the error record (`code`, `message`,
        /// `line`) to this name inside the recovery block.
        error_name: Option<String>,
        line: u32,
    },
    Return(Option<Expr>, u32),
    /// `guard <condition>` — an executed assertion. It traps with
    /// E_KAZE_GUARD when false, and it is the way a script tells the checker
    /// about an invariant it maintains by construction (for example that a
    /// cursor it just advanced is still inside the list).
    Guard(Expr, u32),
}

impl Stmt {
    pub fn line(&self) -> u32 {
        match self {
            Stmt::Let(_, _, l)
            | Stmt::Set(_, _, l)
            | Stmt::Eval(_, l)
            | Stmt::ForEach { line: l, .. }
            | Stmt::While { line: l, .. }
            | Stmt::Try { line: l, .. }
            | Stmt::Guard(_, l)
            | Stmt::Return(_, l) => *l,
            Stmt::If { line, .. } => *line,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordField {
    pub name: String,
    pub type_name: Option<String>,
}

#[derive(Clone, Debug)]
pub enum ItemKind {
    /// `action` — a routine that performs work.
    Action,
    /// `check` — a routine that inspects a machine's state.
    Check,
    /// `filter` — a routine that must end in `allow` or `reject`.
    Filter,
    /// `when <condition>` — fires on every host tick whose poll is true.
    When(Expr),
    /// `every <period>` — fires on ticks where the period divides.
    Every(f64),
    /// `record` — a user-defined record layout.
    Record(Vec<RecordField>),
}

/// A type as written in a signature (Phase 4). Types are checked, never
/// carried into the artifact: Kaze values are dynamically tagged, and a
/// signature type is an obligation on the caller, not a runtime tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeRef {
    Int,
    Number,
    Text,
    Bool,
    /// `percent` — a number in `0..=100`.
    Percent,
    /// Any unit of length; the checker checks the dimension, not the spelling.
    Length,
    Time,
    Angle,
    Temperature,
    /// A declared `record` type, so a routine can say what shape it expects
    /// and the checker can allow field reads on it without a fallback.
    Record(String),
    /// `untrusted<T>` — data that crossed a system boundary and has not yet
    /// been validated by a `requires` clause.
    Untrusted(Box<TypeRef>),
}

impl TypeRef {
    pub fn label(&self) -> &str {
        match self {
            TypeRef::Int => "int",
            TypeRef::Number => "number",
            TypeRef::Text => "text",
            TypeRef::Bool => "true or false",
            TypeRef::Percent => "percent",
            TypeRef::Length => "length",
            TypeRef::Time => "time",
            TypeRef::Angle => "angle",
            TypeRef::Temperature => "temperature",
            TypeRef::Record(name) => name.as_str(),
            TypeRef::Untrusted(_) => "untrusted",
        }
    }

    pub fn parse(word: &str) -> Option<TypeRef> {
        Some(match word {
            "int" => TypeRef::Int,
            "num" | "number" | "float" => TypeRef::Number,
            "str" | "text" => TypeRef::Text,
            "bool" => TypeRef::Bool,
            "percent" => TypeRef::Percent,
            "length" | "meters" => TypeRef::Length,
            "time" | "seconds" => TypeRef::Time,
            "angle" | "degrees" => TypeRef::Angle,
            "temperature" | "Celsius" | "celsius" | "Kelvin" | "kelvin" => TypeRef::Temperature,
            // Anything else names a record; the checker refuses a name that no
            // record declares, so a typo is still an error.
            other => TypeRef::Record(other.to_string()),
        })
    }
}

/// A `requires`/`ensures` clause.
#[derive(Clone, Debug)]
pub struct Clause {
    pub expr: Expr,
    pub line: u32,
}

/// Everything a routine declares about itself beyond its body (Phase 2/4).
#[derive(Clone, Debug, Default)]
pub struct Contracts {
    /// Preconditions, in source order. Checked at every call site the checker
    /// cannot prove; a violating constant argument is a compile-time error.
    pub requires: Vec<Clause>,
    /// Postconditions. `result` names the returned value inside the clause.
    pub ensures: Vec<Clause>,
    /// `recursion bounded <n>` — required for every routine on a call cycle.
    pub recursion_bound: Option<u32>,
    /// Capabilities this routine itself needs. A routine may only use a
    /// capability it declares here, and may only declare one the module has
    /// already declared: the set can only shrink on the way down the call
    /// graph (Phase 5, monotonic attenuation).
    pub routine_needs: Vec<String>,
    /// Parallel to `Item::params`.
    pub param_types: Vec<Option<TypeRef>>,
    /// Parallel to `Item::params`; a default fills in when the 12-factor
    /// resolver finds no CLI flag, environment variable or config entry.
    pub param_defaults: Vec<Option<Expr>>,
    pub result: Option<TypeRef>,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub name: String,
    pub kind: ItemKind,
    pub params: Vec<String>,
    pub body: Vec<Stmt>,
    pub line: u32,
    pub contracts: Contracts,
    /// The file this item came from. `prepare`/`prepare_file` fill it for every
    /// item (the root's own items use the root file name), so diagnostics name
    /// the real file. Empty only for a program built directly from
    /// `parser::parse` without the loader.
    pub origin: String,
}

/// One `use "path"` line. Imports are resolved and inlined before checking and
/// compilation; the compiled module has no imports left.
#[derive(Clone, Debug)]
pub struct Use {
    pub path: String,
    pub line: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Program {
    /// Declared capabilities, exactly as written.
    pub needs: Vec<String>,
    /// `use "path"` directives, in source order. Consumed by the loader.
    pub uses: Vec<Use>,
    /// The root module's diagnostic origin (its file name). The loader sets
    /// this on the flattened program; the compiler carries it into the module
    /// so implicit entry resolution can ignore imported routines.
    pub root: String,
    pub items: Vec<Item>,
    /// Names of the declared records, so a signature type can be checked
    /// against them without walking the item list.
    pub records: Vec<String>,
}

impl Program {
    pub fn item(&self, name: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.name == name)
    }

    pub fn record_fields(&self, name: &str) -> Option<&[RecordField]> {
        for it in &self.items {
            if it.name == name {
                if let ItemKind::Record(ref fields) = it.kind {
                    return Some(fields);
                }
            }
        }
        None
    }

    pub fn is_record(&self, name: &str) -> bool {
        self.record_fields(name).is_some()
    }

    pub fn items_of_kind(&self, want: &ItemKind) -> impl Iterator<Item = &Item> {
        let want_tag = kind_tag(want);
        self.items
            .iter()
            .filter(move |i| kind_tag(&i.kind) == want_tag)
    }
}

fn kind_tag(k: &ItemKind) -> u8 {
    match k {
        ItemKind::Action => 0,
        ItemKind::Check => 1,
        ItemKind::Filter => 2,
        ItemKind::When(_) => 3,
        ItemKind::Every(_) => 4,
        ItemKind::Record(_) => 5,
    }
}
