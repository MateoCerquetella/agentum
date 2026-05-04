#!/bin/bash
# PostToolUse hook: validate amux-server.py syntax after every edit
# Receives JSON on stdin with tool_input.file_path
set -euo pipefail

REPO="/Users/ethan/Dev/amux"
SERVER="$REPO/amux-server.py"

# Read the file path from hook input
FILE_PATH=$(cat | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('tool_input',{}).get('file_path',''))" 2>/dev/null || echo "")

# Only check if amux-server.py was edited
if [ "$FILE_PATH" != "$SERVER" ]; then
  exit 0
fi

# Check Python syntax
if ! python3 -c "import ast; ast.parse(open('$SERVER').read())" 2>/tmp/amux-check-err.txt; then
  echo "Python syntax error in amux-server.py:" >&2
  cat /tmp/amux-check-err.txt >&2
  exit 2  # blocks the action
fi

# Extract JS and check with node
python3 -c "
import re, subprocess, sys, tempfile, os
with open('$SERVER') as f:
    content = f.read()
m = re.search(r'<script>\s*\n(.*?)</script>', content, re.DOTALL)
if not m:
    sys.exit(0)
fd, path = tempfile.mkstemp(suffix='.js')
os.write(fd, m.group(1).encode())
os.close(fd)
r = subprocess.run(['node', '--check', path], capture_output=True, text=True)
os.unlink(path)
if r.returncode != 0:
    print('JS syntax error in amux-server.py:', file=sys.stderr)
    print(r.stderr, file=sys.stderr)
    sys.exit(1)
"
if [ $? -ne 0 ]; then
  exit 2  # blocks the action
fi

exit 0
