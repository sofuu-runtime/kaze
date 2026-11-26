#!/usr/bin/env python3
"""Validate Kaze sources against dist/grammars/kaze.gbnf.

llama.cpp accepts a GBNF grammar as a notation for a context-free language.  Its
validator explores every live stack, which is the right thing for constrained
decoding but is exponential on a whole source file.  This checker is a packrat
(PEG) reader for the same notation: alternatives are tried in order and a
repetition is greedy.  Every match it reports is therefore a real derivation of
the GBNF, so acceptance is sound for the gate; the grammar is written so the
ordered choice does not need to backtrack across statements.

    python3 scripts/gbnf-validate.py dist/grammars/kaze.gbnf lib/*.kaze

Exit status is 0 only when every file matches.  The GBNF itself is checked for
undefined rule references first, so a stale grammar fails even when the sample
files happen to be permissive.
"""
import sys

# --------------------------------------------------------------------------
# GBNF grammar
# --------------------------------------------------------------------------


class Lit:
    __slots__ = ("text",)

    def __init__(self, text):
        self.text = text


class Ref:
    __slots__ = ("name",)

    def __init__(self, name):
        self.name = name


class Cls:
    __slots__ = ("negated", "ranges")

    def __init__(self, negated, ranges):
        self.negated = negated
        self.ranges = ranges

    def matches(self, ch):
        hit = any(lo <= ch <= hi for lo, hi in self.ranges)
        return not hit if self.negated else hit


class Seq:
    __slots__ = ("items",)

    def __init__(self, items):
        self.items = items


class Alt:
    __slots__ = ("items",)

    def __init__(self, items):
        self.items = items


class Rep:
    __slots__ = ("item", "minimum", "maximum")

    def __init__(self, item, minimum, maximum):
        self.item = item
        self.minimum = minimum
        self.maximum = maximum


class GrammarError(Exception):
    pass


def strip_comment(line):
    """Drop a # comment that is outside a string or character class."""
    out = []
    in_string = False
    in_class = False
    escaped = False
    for ch in line:
        if in_string:
            out.append(ch)
            if escaped:
                escaped = False
            elif ch == "\\":
                escaped = True
            elif ch == '"':
                in_string = False
            continue
        if in_class:
            out.append(ch)
            if ch == "]":
                in_class = False
            continue
        if ch == '"':
            in_string = True
            out.append(ch)
        elif ch == "[":
            # A literal "[" at the start of a character class.  The grammar
            # never uses a bare "[" as a literal outside a class.
            in_class = True
            out.append(ch)
        elif ch == "#":
            break
        else:
            out.append(ch)
    return "".join(out)


class GrammarParser:
    """Read the one-rule-per-line GBNF used by kaze.gbnf.

    llama.cpp's reader treats a leading "|" on the next line as a new rule, so
    the shipped grammar keeps every rule on one line; this parser follows the
    same convention and makes the line boundary explicit rather than letting
    whitespace join two rules.
    """

    def __init__(self, text):
        self.lines = text.splitlines()
        self.text = ""
        self.pos = 0
        self.rules = {}

    def parse(self):
        for number, raw in enumerate(self.lines, 1):
            line = strip_comment(raw).strip()
            if not line:
                continue
            head, sep, body = line.partition("::=")
            if not sep:
                raise GrammarError("line %d: expected ::= in %r" % (number, line))
            name = head.strip()
            if not name or not all(c.isalnum() or c == "-" for c in name):
                raise GrammarError("line %d: bad rule name %r" % (number, name))
            if name in self.rules:
                raise GrammarError("line %d: rule %s is defined twice" % (number, name))
            self.text = body
            self.pos = 0
            self.rules[name] = self.parse_alternates()
            self.skip_space()
            if self.pos != len(self.text):
                raise GrammarError(
                    "line %d: trailing text %r in rule %s"
                    % (number, self.text[self.pos :], name)
                )
        if "root" not in self.rules:
            raise GrammarError("the grammar has no root rule")
        self.check_refs()
        return self.rules

    def check_refs(self):
        seen = set()
        stack = [self.rules["root"]]
        while stack:
            node = stack.pop()
            if isinstance(node, Ref):
                if node.name not in self.rules:
                    raise GrammarError("undefined rule reference %s" % node.name)
                if node.name not in seen:
                    seen.add(node.name)
                    stack.append(self.rules[node.name])
            elif isinstance(node, (Seq, Alt)):
                stack.extend(node.items)
            elif isinstance(node, Rep):
                stack.append(node.item)
            elif isinstance(node, Cls):
                pass
            elif not isinstance(node, Lit):
                raise GrammarError("unknown grammar node %r" % (node,))

    def skip_space(self):
        text = self.text
        while self.pos < len(text):
            c = text[self.pos]
            if c in " \t\r\n":
                self.pos += 1
            elif c == "#":
                while self.pos < len(text) and text[self.pos] != "\n":
                    self.pos += 1
            else:
                break

    def parse_name(self):
        start = self.pos
        while self.pos < len(self.text):
            c = self.text[self.pos]
            if c.isalnum() or c == "-":
                self.pos += 1
            else:
                break
        if self.pos == start:
            raise GrammarError("expected a rule name at %r" % self.text[self.pos : self.pos + 20])
        return self.text[start : self.pos]

    def parse_alternates(self):
        items = [self.parse_sequence()]
        self.skip_space()
        while self.pos < len(self.text) and self.text[self.pos] == "|":
            self.pos += 1
            self.skip_space()
            items.append(self.parse_sequence())
            self.skip_space()
        return items[0] if len(items) == 1 else Alt(items)

    def parse_sequence(self):
        items = []
        while True:
            self.skip_space()
            if self.pos >= len(self.text):
                break
            c = self.text[self.pos]
            if c in "|)":
                break
            items.append(self.parse_factor())
        if not items:
            return Seq([])
        return items[0] if len(items) == 1 else Seq(items)

    def parse_factor(self):
        start = self.pos
        c = self.text[self.pos]
        if c == "(":
            self.pos += 1
            inner = self.parse_alternates()
            self.skip_space()
            if self.pos >= len(self.text) or self.text[self.pos] != ")":
                raise GrammarError("unclosed group at offset %d" % start)
            self.pos += 1
            node = inner
        elif c == '"':
            node = Lit(self.parse_string())
        elif c == "[":
            node = self.parse_class()
        else:
            node = Ref(self.parse_name())
        if self.pos < len(self.text):
            c = self.text[self.pos]
            if c == "*":
                self.pos += 1
                return Rep(node, 0, None)
            if c == "+":
                self.pos += 1
                return Rep(node, 1, None)
            if c == "?":
                self.pos += 1
                return Rep(node, 0, 1)
        return node

    def parse_string(self):
        assert self.text[self.pos] == '"'
        self.pos += 1
        out = []
        while self.pos < len(self.text):
            c = self.text[self.pos]
            if c == '"':
                self.pos += 1
                return "".join(out)
            if c == "\\":
                self.pos += 1
                if self.pos >= len(self.text):
                    raise GrammarError("unterminated escape")
                e = self.text[self.pos]
                self.pos += 1
                out.append(
                    {
                        "n": "\n",
                        "r": "\r",
                        "t": "\t",
                        '"': '"',
                        "\\": "\\",
                        "[": "[",
                        "]": "]",
                        "-": "-",
                    }.get(e, e)
                )
            else:
                out.append(c)
                self.pos += 1
        raise GrammarError("unterminated string")

    def parse_class(self):
        assert self.text[self.pos] == "["
        self.pos += 1
        negated = False
        if self.pos < len(self.text) and self.text[self.pos] == "^":
            negated = True
            self.pos += 1
        ranges = []
        while self.pos < len(self.text):
            if self.text[self.pos] == "]":
                self.pos += 1
                return Cls(negated, ranges)
            lo = self.parse_class_char()
            hi = lo
            if self.pos + 1 < len(self.text) and self.text[self.pos] == "-" and self.text[self.pos + 1] != "]":
                self.pos += 1
                hi = self.parse_class_char()
            ranges.append((lo, hi))
        raise GrammarError("unterminated character class")

    def parse_class_char(self):
        c = self.text[self.pos]
        if c == "\\":
            self.pos += 1
            e = self.text[self.pos]
            self.pos += 1
            if e == "x":
                digits = self.text[self.pos : self.pos + 2]
                self.pos += 2
                return chr(int(digits, 16))
            return {"n": "\n", "r": "\r", "t": "\t"}.get(e, e)
        self.pos += 1
        return c


# --------------------------------------------------------------------------
# Packrat matcher
# --------------------------------------------------------------------------


class Matcher:
    def __init__(self, rules, text):
        self.rules = rules
        self.text = text
        self.memo = {}
        self.furthest = 0
        self.furthest_rule = None
        self.max_steps = 40_000_000
        self.steps = 0

    def match_rule(self, name, pos):
        key = (name, pos)
        if key in self.memo:
            return self.memo[key]
        self.steps += 1
        if self.steps > self.max_steps:
            raise RuntimeError("grammar match exceeded the step budget")
        result = self.match(self.rules[name], pos)
        self.memo[key] = result
        if pos > self.furthest:
            self.furthest = pos
            self.furthest_rule = name
        return result

    def match(self, node, pos):
        if isinstance(node, Lit):
            text = self.text
            end = pos + len(node.text)
            if text.startswith(node.text, pos):
                return end
            return -1
        if isinstance(node, Cls):
            if pos < len(self.text) and node.matches(self.text[pos]):
                return pos + 1
            return -1
        if isinstance(node, Ref):
            return self.match_rule(node.name, pos)
        if isinstance(node, Seq):
            cur = pos
            for item in node.items:
                if cur < 0:
                    return -1
                cur = self.match(item, cur)
            return cur
        if isinstance(node, Alt):
            for item in node.items:
                end = self.match(item, pos)
                if end >= 0:
                    return end
            return -1
        if isinstance(node, Rep):
            cur = pos
            count = 0
            while node.maximum is None or count < node.maximum:
                end = self.match(node.item, cur)
                if end < 0 or end == cur:
                    break
                cur = end
                count += 1
            if count < node.minimum:
                return -1
            return cur
        raise RuntimeError("unknown grammar node %r" % (node,))


def main(argv):
    if len(argv) < 3:
        sys.stderr.write("usage: gbnf-validate.py <grammar.gbnf> <file.kaze>...\n")
        return 2
    grammar_path = argv[1]
    try:
        rules = GrammarParser(open(grammar_path, "r", encoding="utf-8").read()).parse()
    except (GrammarError, OSError) as exc:
        sys.stderr.write("kaze.gbnf: %s\n" % exc)
        return 1
    failures = 0
    for path in argv[2:]:
        try:
            text = open(path, "r", encoding="utf-8").read()
        except OSError as exc:
            sys.stderr.write("%s: %s\n" % (path, exc))
            failures += 1
            continue
        matcher = Matcher(rules, text)
        try:
            end = matcher.match_rule("root", 0)
        except RuntimeError as exc:
            sys.stderr.write("%s: %s\n" % (path, exc))
            failures += 1
            continue
        if end == len(text):
            continue
        pos = min(matcher.furthest, len(text))
        line = text.count("\n", 0, pos) + 1
        col = pos - text.rfind("\n", 0, pos)
        snippet = text[pos : pos + 60].split("\n", 1)[0]
        sys.stderr.write(
            "%s: line %d, column %d (in %s): %r\n"
            % (path, line, col, matcher.furthest_rule, snippet)
        )
        failures += 1
    if failures:
        sys.stderr.write("gbnf-validate: %d of %d file(s) do not match\n" % (failures, len(argv) - 2))
        return 1
    print("gbnf-validate: %d file(s) match %s" % (len(argv) - 2, grammar_path))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
