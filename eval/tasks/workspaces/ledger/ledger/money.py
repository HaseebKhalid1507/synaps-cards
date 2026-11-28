"""Amounts in integer cents: parsing, formatting and splitting."""

import re

_AMOUNT = re.compile(r"^(-)?(\d{1,3}(?:,\d{3})*|\d+)(?:\.(\d{1,2}))?$")


def parse(text):
    """The amount `text` names, in cents: `12.50`, `-3.05`, `1,234.00`, or
    `(12.50)` for a negative amount, as statements print them."""
    s = text.strip()
    negative = False
    if s.startswith("(") and s.endswith(")"):
        negative, s = True, s[1:-1].strip()
    m = _AMOUNT.match(s)
    if not m:
        raise ValueError(f"not an amount: {text!r}")
    whole = int(m.group(2).replace(",", ""))
    frac = (m.group(3) or "0").ljust(2, "0")
    cents = whole * 100 + int(frac)
    if m.group(1):
        negative = not negative
    return -cents if negative else cents


def format(cents):
    """`cents` as text: `12.50`, `-0.05`, `1,234.00`."""
    sign = "-" if cents < 0 else ""
    whole, frac = divmod(abs(cents), 100)
    return f"{sign}{whole:,}.{frac:02d}"


def split(cents, parts):
    """`cents` split into `parts` near-equal amounts that sum to it; the
    first ones take the remainder."""
    if parts < 1:
        raise ValueError("split into at least one part")
    base, rest = divmod(cents, parts)
    return [base + (1 if i < rest else 0) for i in range(parts)]
