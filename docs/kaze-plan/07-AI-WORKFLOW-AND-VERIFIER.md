# 07. AI Workflow, Compiler-as-a-Verifier & Automated Repair

> **Status:** Architecture Specification (Workstream W3)  
> **Core Objective:** Establish Kaze as the most reliable programming language for AI agents by turning the strict compiler into an automated verification engine.

---

## 1. Why Existing Languages Fail with AI Agents

When an LLM writes code in Python, C, or JavaScript:
1. **Hallucinated Methods:** The model calls functions that don't exist, which only fail when executed in production.
2. **Missing Boundary Checks:** The model writes `items[0]` without checking if the list is empty, crashing the program.
3. **Unit Confusion:** The model adds milliseconds to seconds or feet to meters without noticing.
4. **Security Vulnerabilities:** The model inadvertently grants elevated authority or leaves open command-injection paths.

Because conventional compilers are permissive, they produce binaries that "look right" but fail during execution.

---

## 2. Kaze: The Infallible AI Verifier

Kaze is designed from the ground up to be the **ideal target language for generative AI**:

```mermaid
sequenceDiagram
    participant User as Human Operator / Task
    participant LLM as AI Agent (Claude / Gemini)
    participant Compiler as Kaze Compiler (Verifier)
    participant Exec as Verified Native Binary

    User->>LLM: "Survey perimeter at 15m altitude and log anomalies"
    LLM->>Compiler: Proposes pure Kaze code (`agent.kaze`)
    Note over Compiler: 1. Checks Syntax & Capabilities<br/>2. Validates Unit Algebra (15m)<br/>3. Verifies Range Contracts<br/>4. Checks Zero-Panic Invariants
    alt Contains Error / Hallucination
        Compiler-->>LLM: Machine-Readable Diagnostic (W2)<br/>[Line 4: 'needs: drone' required; alt must be <= 120m]
        Note over LLM: LLM fixes code autonomously<br/>(Self-Repair Loop)
        LLM->>Compiler: Submits corrected Kaze code
    end
    Compiler->>User: Renders reviewed Intent & Contracts for approval
    User->>Exec: Authorizes execution
    Note over Exec: 100% Guaranteed Safe Execution<br/>(Zero runtime panics or crashes)
```

---

## 3. The Autonomous Self-Repair Loop

When an AI generates Kaze code that violates a contract or type constraint, the Kaze compiler does not output messy human text. It produces **structured, machine-readable diagnostic envelopes (Workstream W2)**:

```json
{
  "code": "E_KAZE_DIMENSION_MISMATCH",
  "line": 12,
  "col": 18,
  "expected": "num<meters>",
  "actual": "num<seconds>",
  "message": "Cannot assign time in seconds to altitude parameter",
  "fix_suggestion": "Use a distance unit, e.g. 15 meters"
}
```

The AI reads this precise feedback and corrects its code in a single turn. The compiler acts as a **rigorous formal filter** before a single physical action can take place.

---

## 4. Reviewable Intent & Human Authorization

An AI must **never** be allowed to grant itself authority:
* **Explicit Capability Review:** The generated script clearly lists its requested authority at the top (`needs: drone, sensor, log`). A human reviewer or security policy engine can inspect that single line and understand the entire blast radius of the agent.
* **No Hidden Imports:** Because Kaze agents are 100% self-contained pure Kaze without auxiliary script wrappers, what you see is strictly what executes.

---

## 5. Local Weights & Zero-Download Governance

In accordance with project rules (`AGENTS.md`):
* All AI evaluation, benchmarking, and repair testing must run against local weights already present on the machine (e.g. `~/Downloads/*.gguf`).
* Never download external weights or background models during test execution.

---

## 6. Implementation Milestones

- [ ] **W3.DiagnosticsJSON:** Implement `--json` output flag in `kaze check` providing machine-readable error envelopes.
- [ ] **W3.RepairCorpus:** Create automated test suite exercising AI self-repair on synthetic contract-violating scripts.
- [ ] **W3.AuditLedger:** Implement cryptographically signed verification ledgers linking AI source code to verified compiler hashes.
