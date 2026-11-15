"""Slot order and 32-bit stack-pop bytes of every Steam interface version in
Proton's i386 lsteamclient.dll (from its symbols: _win<Iface>_<Version>_vtable
and _win<Iface>_<Version>_<Method>), for versions our tables lack.

usage: python3 -I lsc_layouts.py <lsteamclient.dll> <nm output> <tables.rs> <tables32.rs>
prints JSON: {version: [kind, [[method, bytes], ...]]}
"""
import json, re, struct, subprocess, sys

dll, nm_path, tables, tables32 = sys.argv[1:5]
data = open(dll, 'rb').read()
pe = struct.unpack_from('<I', data, 0x3c)[0]
nsec = struct.unpack_from('<H', data, pe + 6)[0]
optsz = struct.unpack_from('<H', data, pe + 20)[0]
base = struct.unpack_from('<I', data, pe + 24 + 28)[0]
secs = []
for i in range(nsec):
    s = pe + 24 + optsz + i * 40
    vsize, rva, rawsz, rawptr = struct.unpack_from('<IIII', data, s + 8)
    secs.append((rva, max(vsize, rawsz), rawptr, rawsz))

def read_u32(va):
    rva = va - base
    for r, sz, ptr, rawsz in secs:
        if r <= rva < r + sz:
            off = rva - r
            return struct.unpack_from('<I', data, ptr + off)[0] if off + 4 <= rawsz else 0
    return None

text = {}   # addr -> [names]
vtables = {}
for line in open(nm_path):
    p = line.split()
    if len(p) != 3:
        continue
    addr, kind, name = int(p[0], 16), p[1], p[2]
    m = re.fullmatch(r'_win(ISteam\w+?)_(\w+)_vtable', name)
    if m:
        vtables[name] = (addr, m.group(1), m.group(2))
    elif kind in 'Tt' and name.startswith('_win'):
        text.setdefault(addr, []).append(name)
starts = sorted(text)

def ret_bytes(addr):
    """Stack bytes popped by the function at addr: the immediate of its ret."""
    nxt = next((a for a in starts if a > addr), addr + 0x400)
    out = subprocess.run(['objdump', '-d', '--start-address=%#x' % addr, '--stop-address=%#x' % nxt, dll],
                         capture_output=True, text=True).stdout
    rets = [int(m.group(1), 16) if m.group(1) else 0 for m in re.finditer(r'\tret\s*(?:\$0x([0-9a-f]+))?\s*$', out, re.M)]
    return max(rets) if rets else None

have = set(re.findall(r'^\s*\("([^"]+)",', open(tables).read(), re.M)) & set(re.findall(r'^\s*\("([^"]+)",', open(tables32).read(), re.M))
result = {}
for sym, (addr, kind, version) in sorted(vtables.items()):
    if version in have and not __import__("os").environ.get("ALL"):
        continue
    methods = []
    prefix = '_win%s_%s_' % (kind, version)
    # Proton shares identical thunks between versions: any version's
    # function of the same interface counts (this version's name first).
    others = ['_win%s_%s_' % (kind, v) for (_, k, v) in vtables.values() if k == kind]
    va = addr
    while True:
        f = read_u32(va)
        names = text.get(f, []) if f else []
        name = next((n[len(prefix):] for n in names if n.startswith(prefix)), None)
        if name is None:
            name = next((n[len(o):] for n in names for o in sorted(others, key=len, reverse=True) if n.startswith(o)), None)
        if name is None:
            break
        b = ret_bytes(f)
        methods.append([name, b if b is not None else 0xFFFF])
        va += 4
    if methods:
        result[version] = [kind, methods]
json.dump(result, sys.stdout, indent=0)
