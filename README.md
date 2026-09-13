# WORM-Q

Linear quantum circuit verifier with JOVIAL/HAL-S-inspired declarative syntax.

Write-Once-Read-Once semantics enforced by the type system. Every qubit handle has exactly one owner; measurement is destructive and consumes the handle. The compiler rejects aliasing, stale-handle reuse, and qubit leaks at program end.

## Build

```
cargo build --release
```

## Usage

```
./target/release/wormq examples/bell.wq
```

## Language

```
QBIT name;          -- declare a linear qubit resource
H    q;             -- Hadamard
X    q;             -- Pauli X
Y    q;             -- Pauli Y
Z    q;             -- Pauli Z
S    q;             -- Phase gate
T    q;             -- T gate
RZ   angle, q;      -- Z-rotation by angle (radians)
CX   ctrl, target;  -- CNOT (entangling, consumes and rebinds both)
SWAP a, b;          -- SWAP (consumes and rebinds both)
CCX  c1, c2, t;     -- Toffoli (consumes and rebinds all three)
MZ   q;             -- Destructive Z-basis measurement (terminates the handle)
END;                -- Verify no live handles remain, emit SSA IR
```

Comments begin with `--`.

## Safety properties

| Property | How it is enforced |
|---|---|
| No cloning | `limited` ownership: each handle has one live binding |
| Destructive read | `MZ` consumes the handle; no successor wire exists |
| No aliasing | Multi-qubit gates check pairwise distinctness before consuming |
| No leaks | `END` rejects programs with any live, unmeasured handles |
| No stale reuse | `take()` fails if handle is already `Consumed` |

## Emitted IR

The verifier emits a typed linear SSA listing after successful verification:

```
; typed linear SSA
%q0 = alloc0 ; A
%q1 = alloc0 ; B
%q2 = h %q0
%q3, %q4 = cx %q2, %q1
%b3 = mz %q3
%b4 = mz %q4
```

Each `%qN` value has exactly one definition and one consuming use. `%bN` values are classical measurement outcomes.

## Architecture

Inspired by JOVIAL (1959, aerospace embedded) and HAL/S (1970s, NASA Shuttle) — languages that favored explicit static allocation, bounded state, and auditability over ergonomics. Applied here to linear quantum wires: the state is bounded (no dynamic qubit allocation at runtime), the constraints are structural (the compiler, not the runtime, rejects violations), and the output is inspectable SSA.

The two-tier WORO/QEC architecture this compiler targets:

- **Tier 1 (Logical core)**: protected qubits under continuous QEC, never directly measured
- **Tier 2 (Ancilla interface)**: transient worker qubits with explicit WORO semantics — entangled briefly to extract a syndrome, then destructively read and discarded

`MZ` in WORM-Q corresponds to the Tier 2 destructive read. The Tier 1 logical state is modeled as the surviving entangled region that has not yet been measured.
