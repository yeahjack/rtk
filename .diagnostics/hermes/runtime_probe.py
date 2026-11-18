"""Exercise the installed, unmodified Hermes terminal implementation, without a model/account.

This is a direct packaged-runtime probe, not a claim of a full desktop chat session.
"""
import ctypes
import json
import os
from pathlib import Path
import site
import sys

root, source, dependencies, repo, *rtks = sys.argv[1:]
site.addsitedir(dependencies)
sys.path.insert(0, source)
size = ctypes.c_uint32(0)
result = ctypes.windll.kernel32.GetCurrentPackageFullName(ctypes.byref(size), None)
print("PACKAGE_IDENTITY_FIRST_RESULT", result, flush=True)
if result == 122:
    buffer = ctypes.create_unicode_buffer(size.value)
    result = ctypes.windll.kernel32.GetCurrentPackageFullName(ctypes.byref(size), buffer)
    print("PACKAGE_IDENTITY", result, buffer.value, flush=True)
print("INSTALLED_PYTHON", sys.executable, flush=True)
print("INSTALLED_SOURCE", source, flush=True)
from pm.environments import activate_dependencies
activate_dependencies(Path(source))
from tools.terminal_tool import terminal_tool
print("ACTUAL_TERMINAL_PATH", terminal_tool("printf '%s\\n' \"$PATH\"; type -a git", workdir=repo, timeout=60), flush=True)
for rtk in rtks:
    command = '"' + rtk.replace('\\', '/') + '" git status'
    print("RTK_COMMAND", command, flush=True)
    print("RTK_TERMINAL_RESULT", terminal_tool(command, workdir=repo, timeout=60), flush=True)
