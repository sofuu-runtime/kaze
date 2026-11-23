#!/usr/bin/env python3
"""Generate random Kaze programs that both engines can run.

Each program returns one number and only uses constructs with defined semantics in
both the interpreter and the C backend. Several flavours are cycled through so the
gate covers arithmetic, text, lists, records, points, conditionals and logging.
Any program where the engines disagree is a defect in one of them.
"""
import random, sys

def flavour(seed):
    return seed % 7

def arithmetic(r):
    a, b = r.randint(-40, 40), r.randint(1, 9)
    body = [
        f"    let a = {a}",
        f"    let b = {b}",
        f"    let m = a * b + {r.randint(-5, 5)}",
        f"    let d = (a + b) / b otherwise 0",
        f"    let q = a % b otherwise 0",
        f"    let n = ((a * a) - (b * b)) + (a / b otherwise 0)",
        "    return m + d + q + n",
    ]
    return body

def text_work(r):
    s = "ab" * r.randint(1, 3) + "z"
    i = r.randint(0, len(s) + 2)
    return [
        f'    let s = "{s}"',
        "    let n = length of s",
        f"    let c = s[{i}] otherwise \"-\"",
        "    let cn = length of c",
        '    let joined = s + "!"',
        "    return n + cn + (length of joined)",
    ]

def list_work(r):
    items = [str(r.randint(-9, 9)) for _ in range(r.randint(2, 6))]
    return [
        "    let xs = [%s]" % ", ".join(items),
        "    let total = 0",
        "    for each x in xs:",
        "        set total to total + x",
        "    let n = length of xs",
        "    let i = 1",
        "    let second = xs[i] otherwise 0",
        "    return total + n + second",
    ]

def record_work(r):
    return [
        "    let o = Outer(value: %d, weight: %d)" % (r.randint(-9, 9), r.randint(1, 5)),
        "    let v = o.value",
        "    let w = o.weight",
        "    return v * w",
    ]

def point_work(r):
    return [
        f"    let p = vector({r.randint(-4,4)}, {r.randint(-4,4)}, {r.randint(-4,4)})",
        f"    let q = vector({r.randint(-4,4)}, {r.randint(-4,4)}, {r.randint(-4,4)})",
        "    let d = distance between p and q",
        "    let sl = slant distance between p and q",
        "    let br = bearing from p to q",
        "    let off = q - p",
        "    let doubled = off * 2",
        "    let ratio = d / sl otherwise 0",
        # a bearing is an angle: use it in a comparison rather than in the sum,
        # because adding an angle to a plain number is a unit mismatch
        "    let north = br < 90 degrees",
        "    let bump = 0",
        "    if north:",
        "        set bump to 1",
        "    return (length of doubled) + ratio + bump",
    ]

def conditional_work(r):
    a = r.randint(-9, 9)
    return [
        f"    let a = {a}",
        "    let out = 0",
        "    if a > 0:",
        "        set out to a * 2",
        "    otherwise:",
        "        set out to (0 - a) + 1",
        "    let flag = a == 0",
        "    if flag:",
        "        set out to out + 100",
        "    return out",
    ]

def capability_denied(r):
    return [
        "    let d = lidar.front_distance() otherwise 5 meters",
        "    return 1",
    ]

HOOK_BODIES = [
    "when 1 > 0:\n    log \"guard\"\n",
    "every 100 ms:\n    log \"tick\"\n",
    "when 2 > 1:\n    log \"guard\"\n    log 7\n",
]

def try_work(r):
    return [
        "    let out = 0",
        "    try:",
        "        set out to %d" % r.randint(-5, 5),
        "    otherwise with e:",
        "        set out to 99",
        "    let after = out + 1",
        "    return after",
    ]

def guard_work(r):
    n = r.randint(1, 4)
    return [
        f"    let i = 0",
        "    let total = 0",
        f"    while i < {n} bounded 10:",
        "        set total to total + i",
        "        set i to i + 1",
        "    guard total >= 0",
        "    guard i > 0",
        "    return total + i",
    ]

def nested_loop_work(r):
    n = r.randint(1, 3)
    m = r.randint(1, 3)
    return [
        "    let i = 0",
        "    let total = 0",
        f"    while i < {n} bounded 20:",
        "        let j = 0",
        f"        while j < {m} bounded 20:",
        "            set total to total + (i * j)",
        "            set j to j + 1",
        "        set i to i + 1",
        "    return total",
    ]

def hooks_work(r):
    return [
        "    log \"entry\"",
        "    return 1",
    ]

def log_work(r):
    a = r.randint(-20, 20)
    b = r.randint(1, 7)
    return [
        f"    let a = {a}",
        f"    let b = {b}",
        '    log "sum"',
        "    log a + b",
        '    log "scaled"',
        "    log (a * b) / b otherwise 0",
        "    return a + b",
    ]

HEADERS = {
    3: "record Outer:\n    value: num\n    weight: num\n\n",
    6: "needs: log\n\n",
    7: "needs: lidar\n\n",
    8: "needs: log\n\n",
}

def program(seed):
    r = random.Random(seed)
    flavour_id = seed % 12
    maker = [
        arithmetic,
        text_work,
        list_work,
        record_work,
        point_work,
        conditional_work,
        log_work,
        capability_denied,
        hooks_work,
        try_work,
        guard_work,
        nested_loop_work,
    ][flavour_id]
    body = maker(r)
    head = HEADERS.get(flavour_id, "")
    text = head + "action main() -> num:\n" + "\n".join(body) + "\n"
    if flavour_id == 8:
        text += "\n" + HOOK_BODIES[r.randrange(len(HOOK_BODIES))]
    return text

if __name__ == "__main__":
    seed = int(sys.argv[1]) if len(sys.argv) > 1 else 1
    sys.stdout.write(program(seed))