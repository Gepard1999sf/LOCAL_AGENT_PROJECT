#!/usr/bin/env python3
"""Symbolic math helper behind the ai-drive MCP tool `math_eval`.

Usage: python -B sympy_helper.py <mode> "<expr>" [var]
  mode: diff | gradient3d | integrate | simplify | solve
  expr: SymPy/Python syntax; `^` is accepted as power.
  var:  differentiation / integration / solve variable (default t)

Prints exactly one JSON object on stdout. Exit code 1 with {"error": ...} on failure.
The GDScript/GLSL rendering prints every number as a float (2.0*x, pow(x, 2.0)) so it pastes
into a .gd or .gdshader without int/float type errors; `PI` is the Godot constant.

Security: the expression comes from the language model, and SymPy's parser evaluates Python.
Only characters of a math expression are accepted (no quotes, brackets, attribute access or
dunder names) and the evaluation namespace has no builtins, so `__import__` and friends are
unreachable even if a prompt injection asks for them.
"""

import json
import re
import sys

try:
    import sympy as sp
    from sympy.parsing.sympy_parser import (
        convert_xor,
        implicit_multiplication_application,
        parse_expr,
        standard_transformations,
    )
    from sympy.printing.c import C99CodePrinter
except ImportError:  # pragma: no cover - depends on the user's machine
    print(json.dumps({"error": "SymPy не установлен: выполните pip install sympy"}, ensure_ascii=False))
    sys.exit(1)

MODES = ("diff", "gradient3d", "integrate", "simplify", "solve")
SYMBOL_NAMES = (
    "x y z t k m g v0 omega delta a b c d h r s u v w A B L R T "
    "phi theta alpha beta gamma lam mu rho sigma tau"
)
TRANSFORMATIONS = standard_transformations + (convert_xor, implicit_multiplication_application)
# Letters, digits, operators, parentheses, commas, whitespace and decimal points only.
ALLOWED = re.compile(r"^[0-9A-Za-z_+\-*/^(),\s.]+$")
FORBIDDEN_WORDS = {"lambda", "import", "exec", "eval", "open", "getattr", "setattr", "globals", "locals", "compile", "input", "print", "exit", "quit", "breakpoint", "del", "class", "def", "yield", "assert", "raise", "with", "try", "while", "for", "if", "else", "return", "pass", "global", "nonlocal", "await", "async", "from", "as", "is", "in", "not", "and", "or"}


class GodotPrinter(C99CodePrinter):
    """C99 output tweaked for GDScript 2.0 and Godot shaders: float literals everywhere,
    abs()/PI instead of fabs()/M_PI."""

    def __init__(self):
        super().__init__({"user_functions": {"Abs": "abs"}})

    def _print_Integer(self, expr):
        return f"{int(expr)}.0"

    def _print_Rational(self, expr):
        return f"({expr.p}.0 / {expr.q}.0)"

    def _print_Pi(self, expr):
        return "PI"

    def _print_Exp1(self, expr):
        return "exp(1.0)"

    def _print_Float(self, expr):
        text = super()._print_Float(expr)
        return text if ("." in text or "e" in text) else f"{text}.0"


def godot_code(expr):
    return GodotPrinter().doprint(expr)


def build_symbols():
    return {name: sp.Symbol(name, real=True) for name in SYMBOL_NAMES.split()}


def validate(expr_str):
    if len(expr_str) > 2000:
        raise ValueError("выражение длиннее 2000 символов")
    if not expr_str.strip():
        raise ValueError("пустое выражение")
    if not ALLOWED.match(expr_str):
        raise ValueError("в выражении допустимы только буквы, цифры, + - * / ^ ( ) , и точка в числах")
    if "__" in expr_str:
        raise ValueError("имена с __ недопустимы")
    # A dot is only a decimal point: `1.5`, `.5`, `2.`; never attribute access like `x.real`.
    if re.search(r"[A-Za-z_)]\s*\.|\.\s*[A-Za-z_(]", expr_str):
        raise ValueError("точка допустима только в числах (0.5), доступ к атрибутам запрещён")
    words = set(re.findall(r"[A-Za-z_][A-Za-z0-9_]*", expr_str))
    bad = sorted(words & FORBIDDEN_WORDS)
    if bad:
        raise ValueError(f"недопустимые слова: {', '.join(bad)}")


def sympy_namespace():
    """`from sympy import *` without Python builtins, so nothing outside SymPy is callable."""
    ns = {}
    exec("from sympy import *", ns)  # noqa: S102 - fixed literal, fills the namespace
    ns["__builtins__"] = {}
    return ns


def parse(expr_str, symbols):
    validate(expr_str)
    return parse_expr(expr_str, local_dict=symbols, global_dict=sympy_namespace(), transformations=TRANSFORMATIONS, evaluate=True)


def solve_task(mode, expr_str, var_str):
    if mode not in MODES:
        raise ValueError(f"неизвестный режим {mode!r}; допустимо: {', '.join(MODES)}")
    if not re.match(r"^[A-Za-z_][A-Za-z0-9_]*$", var_str) or "__" in var_str:
        raise ValueError(f"недопустимое имя переменной {var_str!r}")
    symbols = build_symbols()
    expr = parse(expr_str, symbols)
    var = symbols.get(var_str) or sp.Symbol(var_str, real=True)
    result = {"mode": mode, "input": str(expr), "var": str(var)}

    if mode == "gradient3d":
        grad = [sp.simplify(sp.diff(expr, symbols[axis])) for axis in ("x", "y", "z")]
        result["symbolic"] = [str(g) for g in grad]
        result["latex"] = [sp.latex(g) for g in grad]
        result["gdscript_glsl_ready"] = "Vector3({}, {}, {})".format(*(godot_code(g) for g in grad))
        result["note"] = "Для нормали SDF: normalize(gradient). Godot: Y вверх, -Z вперёд."
        return result

    if mode == "solve":
        solutions = sp.solve(sp.Eq(expr, 0), var)
        result["symbolic"] = [str(s) for s in solutions]
        result["latex"] = [sp.latex(s) for s in solutions]
        result["gdscript_glsl_ready"] = [godot_code(s) for s in solutions]
        return result

    if mode == "diff":
        res = sp.simplify(sp.diff(expr, var))
    elif mode == "integrate":
        res = sp.simplify(sp.integrate(expr, var))
    else:
        res = sp.simplify(expr)
    result["symbolic"] = str(res)
    result["latex"] = sp.latex(res)
    result["gdscript_glsl_ready"] = godot_code(res)
    return result


def main(argv):
    if len(argv) < 2:
        print(json.dumps({"error": "использование: sympy_helper.py <mode> <expr> [var]"}, ensure_ascii=False))
        return 2
    mode, expr_str = argv[0], argv[1]
    var_str = argv[2] if len(argv) > 2 else "t"
    try:
        print(json.dumps(solve_task(mode, expr_str, var_str), ensure_ascii=False))
        return 0
    except Exception as exc:  # noqa: BLE001 - everything becomes a JSON error for the agent
        print(json.dumps({"error": f"{type(exc).__name__}: {exc}"}, ensure_ascii=False))
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
