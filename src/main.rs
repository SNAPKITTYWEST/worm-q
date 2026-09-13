use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::process;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Live,
    Consumed,
}

#[derive(Clone, Debug)]
struct Wire {
    ssa: usize,
    state: State,
}

#[derive(Debug)]
struct Compiler {
    wires: BTreeMap<String, Wire>,
    next_ssa: usize,
    ir: Vec<String>,
    line: usize,
}

impl Compiler {
    fn new() -> Self {
        Self {
            wires: BTreeMap::new(),
            next_ssa: 0,
            ir: Vec::new(),
            line: 0,
        }
    }

    fn fail<T>(&self, message: impl AsRef<str>) -> Result<T, String> {
        Err(format!("line {}: {}", self.line, message.as_ref()))
    }

    fn fresh(&mut self) -> usize {
        let id = self.next_ssa;
        self.next_ssa += 1;
        id
    }

    fn declare(&mut self, name: &str) -> Result<(), String> {
        if self.wires.contains_key(name) {
            return self.fail(format!("duplicate declaration of QBIT {}", name));
        }
        let out = self.fresh();
        self.wires.insert(
            name.to_string(),
            Wire { ssa: out, state: State::Live },
        );
        self.ir.push(format!("%q{} = alloc0 ; {}", out, name));
        Ok(())
    }

    fn take(&mut self, name: &str) -> Result<usize, String> {
        let wire = match self.wires.get_mut(name) {
            Some(w) => w,
            None => return self.fail(format!("unknown QBIT {}", name)),
        };
        if wire.state == State::Consumed {
            return self.fail(format!("QBIT {} was already consumed", name));
        }
        wire.state = State::Consumed;
        Ok(wire.ssa)
    }

    fn replace(&mut self, name: &str, ssa: usize) -> Result<(), String> {
        let wire = match self.wires.get_mut(name) {
            Some(w) => w,
            None => return self.fail(format!("unknown QBIT {}", name)),
        };
        if wire.state != State::Consumed {
            return self.fail(format!(
                "internal linearity error: {} was not consumed before replacement",
                name
            ));
        }
        wire.ssa = ssa;
        wire.state = State::Live;
        Ok(())
    }

    fn require_distinct(&self, names: &[&str]) -> Result<(), String> {
        for i in 0..names.len() {
            for j in (i + 1)..names.len() {
                if names[i] == names[j] {
                    return self.fail(format!(
                        "gate operands must be distinct: {} appears twice",
                        names[i]
                    ));
                }
            }
        }
        Ok(())
    }

    fn unary(&mut self, op: &str, name: &str) -> Result<(), String> {
        let input = self.take(name)?;
        let output = self.fresh();
        self.replace(name, output)?;
        self.ir.push(format!("%q{} = {} %q{}", output, op.to_lowercase(), input));
        Ok(())
    }

    fn measure(&mut self, name: &str) -> Result<(), String> {
        let input = self.take(name)?;
        self.ir.push(format!("%b{} = mz %q{}", input, input));
        Ok(())
    }

    fn swap(&mut self, a: &str, b: &str) -> Result<(), String> {
        self.require_distinct(&[a, b])?;
        let a_in = self.take(a)?;
        let b_in = self.take(b)?;
        let a_out = self.fresh();
        let b_out = self.fresh();
        self.replace(a, a_out)?;
        self.replace(b, b_out)?;
        self.ir.push(format!(
            "%q{}, %q{} = swap %q{}, %q{}",
            a_out, b_out, a_in, b_in
        ));
        Ok(())
    }

    fn ccx(&mut self, c1: &str, c2: &str, target: &str) -> Result<(), String> {
        self.require_distinct(&[c1, c2, target])?;
        let c1_in = self.take(c1)?;
        let c2_in = self.take(c2)?;
        let t_in = self.take(target)?;
        let c1_out = self.fresh();
        let c2_out = self.fresh();
        let t_out = self.fresh();
        self.replace(c1, c1_out)?;
        self.replace(c2, c2_out)?;
        self.replace(target, t_out)?;
        self.ir.push(format!(
            "%q{}, %q{}, %q{} = ccx %q{}, %q{}, %q{}",
            c1_out, c2_out, t_out, c1_in, c2_in, t_in
        ));
        Ok(())
    }

    fn cx(&mut self, ctrl: &str, target: &str) -> Result<(), String> {
        self.require_distinct(&[ctrl, target])?;
        let c_in = self.take(ctrl)?;
        let t_in = self.take(target)?;
        let c_out = self.fresh();
        let t_out = self.fresh();
        self.replace(ctrl, c_out)?;
        self.replace(target, t_out)?;
        self.ir.push(format!(
            "%q{}, %q{} = cx %q{}, %q{}",
            c_out, t_out, c_in, t_in
        ));
        Ok(())
    }

    fn rz(&mut self, angle: &str, name: &str) -> Result<(), String> {
        let input = self.take(name)?;
        let output = self.fresh();
        self.replace(name, output)?;
        self.ir.push(format!("%q{} = rz({}) %q{}", output, angle, input));
        Ok(())
    }

    fn finish(&self) -> Result<(), String> {
        let leaks: Vec<String> = self
            .wires
            .iter()
            .filter(|(_, wire)| wire.state == State::Live)
            .map(|(name, wire)| format!("{} (%q{})", name, wire.ssa))
            .collect();
        if leaks.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "program ended with live QBIT resources: {}",
                leaks.join(", ")
            ))
        }
    }

    fn parse_name<'a>(&self, text: &'a str) -> Result<&'a str, String> {
        let name = text.trim().trim_end_matches(';').trim();
        if name.is_empty() {
            return self.fail("missing QBIT name");
        }
        if !name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
            return self.fail(format!("invalid identifier {}", name));
        }
        Ok(name)
    }

    fn parse_args<'a>(&self, text: &'a str, arity: usize) -> Result<Vec<&'a str>, String> {
        let clean = text.trim().trim_end_matches(';').trim();
        let args: Vec<&str> = clean
            .split(',')
            .map(str::trim)
            .filter(|arg| !arg.is_empty())
            .collect();
        if args.len() != arity {
            return self.fail(format!("expected {} operand(s), got {}", arity, args.len()));
        }
        for arg in &args {
            self.parse_name(arg)?;
        }
        Ok(args)
    }

    fn parse_rz_args<'a>(&self, text: &'a str) -> Result<(&'a str, &'a str), String> {
        let clean = text.trim().trim_end_matches(';').trim();
        let mut parts = clean.splitn(2, ',');
        let angle = parts.next().unwrap_or("").trim();
        let q = parts.next().unwrap_or("").trim();
        if q.is_empty() {
            return self.fail("RZ requires: angle, qubit");
        }
        Ok((angle, q))
    }

    fn compile_line(&mut self, raw: &str) -> Result<bool, String> {
        let line = raw.split("--").next().unwrap_or("").trim();
        if line.is_empty() {
            return Ok(false);
        }
        if line.eq_ignore_ascii_case("END;") || line.eq_ignore_ascii_case("END") {
            self.finish()?;
            return Ok(true);
        }
        let mut words = line.splitn(2, char::is_whitespace);
        let opcode = words.next().unwrap().to_ascii_uppercase();
        let rest = words.next().unwrap_or("").trim();

        match opcode.as_str() {
            "QBIT" => self.declare(self.parse_name(rest)?),
            "H"    => { let q = self.parse_name(rest)?; self.unary("H", q) }
            "X"    => { let q = self.parse_name(rest)?; self.unary("X", q) }
            "Y"    => { let q = self.parse_name(rest)?; self.unary("Y", q) }
            "Z"    => { let q = self.parse_name(rest)?; self.unary("Z", q) }
            "S"    => { let q = self.parse_name(rest)?; self.unary("S", q) }
            "T"    => { let q = self.parse_name(rest)?; self.unary("T", q) }
            "MZ"   => { let q = self.parse_name(rest)?; self.measure(q) }
            "SWAP" => { let args = self.parse_args(rest, 2)?; self.swap(args[0], args[1]) }
            "CCX"  => { let args = self.parse_args(rest, 3)?; self.ccx(args[0], args[1], args[2]) }
            "CX"   => { let args = self.parse_args(rest, 2)?; self.cx(args[0], args[1]) }
            "RZ"   => {
                let (angle, q) = self.parse_rz_args(rest)?;
                let q_owned = q.to_string();
                self.rz(angle, &q_owned)
            }
            _ => self.fail(format!("unknown statement {}", opcode)),
        }?;
        Ok(false)
    }

    fn compile(&mut self, source: &str) -> Result<(), String> {
        let mut saw_end = false;
        for (index, raw) in source.lines().enumerate() {
            self.line = index + 1;
            if saw_end && !raw.trim().is_empty() {
                return self.fail("text found after END");
            }
            if self.compile_line(raw)? {
                saw_end = true;
            }
        }
        if !saw_end {
            self.line = source.lines().count();
            return self.fail("missing END");
        }
        Ok(())
    }
}

fn usage(program: &str) -> ! {
    eprintln!("usage: {} program.wq", program);
    process::exit(2);
}

fn main() {
    let mut args = env::args();
    let program = args.next().unwrap_or_else(|| "wormq".to_string());
    let path = args.next().unwrap_or_else(|| usage(&program));
    if args.next().is_some() {
        usage(&program);
    }
    let source = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("cannot read {}: {}", path, error);
            process::exit(1);
        }
    };
    let mut compiler = Compiler::new();
    match compiler.compile(&source) {
        Ok(()) => {
            println!("WORM-Q verification succeeded.");
            println!();
            println!("; typed linear SSA");
            for instruction in &compiler.ir {
                println!("{}", instruction);
            }
        }
        Err(error) => {
            eprintln!("WORM-Q verification failed: {}", error);
            process::exit(1);
        }
    }
}
