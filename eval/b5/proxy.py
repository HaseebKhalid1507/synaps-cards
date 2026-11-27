"""The proxy tokenizers (docs/blocks.md §2.8): what the calibration fits to
a provider's counts, where there is no token-count endpoint.

- `cl100k`, `o200k`: OpenAI's BPE tokenizers (tiktoken), the proxies §2.7's
  spot checks used. A real run without an endpoint takes one of them.
- `bytes4`: ⌈UTF-8 bytes / 4⌉, the scripted model's own tokenizer: the dry
  run's, so its fit can be checked against what it must come to. Never a
  real run's.
"""


def bytes4(text):
    return (len(text.encode()) + 3) // 4


def get(name):
    if name == "bytes4":
        return bytes4
    if name in ("cl100k", "o200k"):
        try:
            import tiktoken
        except ImportError:
            raise RuntimeError(f"the {name} proxy needs tiktoken (pip install tiktoken)") from None
        enc = tiktoken.get_encoding(f"{name}_base")
        return lambda text: len(enc.encode(text, disallowed_special=()))
    raise ValueError(f"no proxy {name!r}: cl100k, o200k or bytes4")
