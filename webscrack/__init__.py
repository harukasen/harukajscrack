"""Python API for the Oxc-powered webscrack JavaScript toolkit."""
from ._native import Bundle, Module, Result, deobfuscate, format, minify, transform, unpack, unminify, webcrack

__all__ = ["Bundle", "Module", "Result", "deobfuscate", "format", "minify", "transform", "unpack", "unminify", "webcrack"]
__version__ = "0.2.0"
