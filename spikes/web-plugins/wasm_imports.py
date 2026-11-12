"""Lists a core wasm module's imports and its size: wasm_imports.py <file.wasm>"""
import sys

def leb(b, i):
    r = s = 0
    while True:
        x = b[i]; i += 1
        r |= (x & 0x7f) << s; s += 7
        if x < 0x80:
            return r, i

def name(b, i):
    n, i = leb(b, i)
    return b[i:i+n].decode(), i + n

b = open(sys.argv[1], "rb").read()
print("bytes", len(b))
i = 8
while i < len(b):
    sid = b[i]; i += 1
    size, i = leb(b, i)
    if sid == 2:
        j = i
        count, j = leb(b, j)
        for _ in range(count):
            m, j = name(b, j); f, j = name(b, j)
            kind = b[j]; j += 1
            print("import", m, f, kind)
            if kind == 0: _, j = leb(b, j)
            elif kind == 1: j += 1; fl, j = leb(b, j); _, j = leb(b, j); j = leb(b, j)[1] if fl & 1 else j
            elif kind == 2: fl, j = leb(b, j); _, j = leb(b, j); j = leb(b, j)[1] if fl & 1 else j
            elif kind == 3: j += 2
    i += size
