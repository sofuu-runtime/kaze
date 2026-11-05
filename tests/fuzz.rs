// v0.7: deterministic fuzzing for the parser/checker/compiler and for
// malformed `.kazebc` modules. No external fuzz dependency; the seed is fixed,
// so a failure is reproducible. The assertion is "no panic": the test harness
// fails a test that unwinds.

use kaze::isa;
use kaze::RunOptions;

/// xorshift64* — deterministic, adequate for byte and character choice.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

fn grants() -> Vec<String> {
    ["drone", "rover", "sensor", "device", "memory", "policy", "log", "alert"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

#[test]
fn mutated_bytecode_never_panics() {
    // Seeds cover try/link, vectors/trig, lists/loops, and a policy filter, so
    // more of the ISA is reachable through the verified path.
    let seeds = [
        include_str!("../examples/fault_tolerance.kaze"),
        include_str!("../examples/kinematics.kaze"),
        include_str!("../examples/patrol.kaze"),
        include_str!("../examples/guardrail.kaze"),
    ];
    let bases: Vec<Vec<u8>> = seeds
        .iter()
        .map(|src| {
            let module = kaze::compile_source(src, "fuzz.kaze").expect("the seed compiles");
            let bytes = isa::serialize(&module);
            assert!(!bytes.is_empty());
            bytes
        })
        .collect();

    let opts = RunOptions { grants: grants(), fuel: 300, ..Default::default() };
    let mut rng = Rng::new(0x9e37_79b9_7f4a_7c15);
    let mut accepted = 0usize;
    let mut executed = 0usize;

    for _ in 0..2500 {
        let mut bytes = bases[rng.below(bases.len())].clone();
        let mutations = 1 + rng.below(8);
        for _ in 0..mutations {
            if bytes.is_empty() {
                break;
            }
            match rng.below(4) {
                0 => {
                    let at = rng.below(bytes.len());
                    bytes[at] ^= (rng.next() & 0xff) as u8;
                }
                1 => {
                    let at = rng.below(bytes.len());
                    bytes[at] = (rng.next() & 0xff) as u8;
                }
                2 => {
                    let at = rng.below(bytes.len());
                    bytes.truncate(at);
                }
                _ => {
                    // Flip a whole little-endian word, which is more likely to
                    // be read as a table count, index or opcode.
                    if bytes.len() >= 4 {
                        let at = rng.below(bytes.len() - 3);
                        let word = (rng.next() & 0xffff_ffff) as u32;
                        bytes[at..at + 4].copy_from_slice(&word.to_le_bytes());
                    }
                }
            }
        }
        if let Ok(module) = isa::deserialize(&bytes) {
            accepted += 1;
            // The artifact passed the verifier; running it must be safe.
            if let Ok(report) = kaze::run_module(module, "<fuzz>", &opts) {
                if report.instructions > 0 {
                    executed += 1;
                }
            }
        }
    }
    // Most mutations corrupt the header/table; the point is that every path
    // returns an error instead of panicking. If none passed, the "accepted
    // artifact runs" half would be empty and the suite would look green for
    // the wrong reason.
    assert!(
        accepted > 0,
        "no mutated artifact passed the verifier; run_module was never exercised"
    );
    assert!(
        executed > 0,
        "no accepted artifact executed an instruction; the VM path is not fuzzed"
    );
}

#[test]
fn random_source_never_panics() {
    let alphabet: &[u8] =
        b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_ \n\t:()[],.\"#+-*/%<>=!";
    let mut rng = Rng::new(0x0123_4567_89ab_cdef);
    for _ in 0..2500 {
        let len = rng.below(240);
        let mut src = String::with_capacity(len);
        for _ in 0..len {
            src.push(alphabet[rng.below(alphabet.len())] as char);
        }
        // The formatter must refuse broken input without panicking too.
        let _ = kaze::fmt::format_source(&src, "<fuzz>");
        if let Ok(program) = kaze::parser::parse(&src) {
            if kaze::checker::check(&program).is_ok() {
                if let Ok(module) = kaze::compiler::compile(&program) {
                    let _ = isa::verify(&module);
                    let opts = RunOptions { fuel: 200, ..Default::default() };
                    let _ = kaze::run_module(module, "<fuzz>", &opts);
                }
            }
        }
    }
}
