"""Update project modules in a matching local PyInstaller worker (Python 3.11).

Preserves the original bootloader and dependency modules; outputs a separate
worker and a hash-pinned compatibility manifest without modifying the base.
"""
import argparse, hashlib, io, json, marshal, struct, sys, zlib
from pathlib import Path


def repack(base: Path, destination: Path):
    from PyInstaller.archive.readers import CArchiveReader
    source_root = Path(__file__).resolve().parents[1]
    archive = CArchiveReader(str(base))
    original = base.read_bytes()
    cookie = struct.unpack(archive._COOKIE_FORMAT, original[archive._end_offset-archive._COOKIE_LENGTH:archive._end_offset])
    assert cookie[4] == sys.version_info.major * 100 + sys.version_info.minor == 311
    data = io.BytesIO()
    entries = []
    replaced = []
    for name, (_, _, _, compressed, kind) in archive.toc.items():
        payload = archive.extract(name)
        if kind == 'z':
            offset, = struct.unpack('!i', payload[8:12])
            toc = dict(marshal.loads(payload[offset:]))
            rebuilt = io.BytesIO(); rebuilt.write(payload[:17]); new_toc = []
            for module, (module_kind, position, length) in toc.items():
                chunk = payload[position:position+length]
                if module == 'ai_worker' or module.startswith('ai_worker.'):
                    relative = module.replace('.', '/') + ('/__init__.py' if module_kind == 1 else '.py')
                    source = source_root / relative
                    assert source.is_file(), source
                    chunk = zlib.compress(marshal.dumps(compile(source.read_text(encoding='utf-8'), relative, 'exec')), 6)
                    replaced.append(module)
                new_toc.append((module, (module_kind, rebuilt.tell(), len(chunk))))
                rebuilt.write(chunk)
            toc_offset = rebuilt.tell(); rebuilt.write(marshal.dumps(new_toc)); rebuilt.seek(8)
            rebuilt.write(struct.pack('!i', toc_offset)); payload = rebuilt.getvalue()
        raw_length = len(payload)
        if compressed: payload = zlib.compress(payload, 9)
        entries.append((data.tell(), len(payload), raw_length, compressed, kind, name))
        data.write(payload)
    assert {'ai_worker.devices','ai_worker.separate','ai_worker.transcribe'} <= set(replaced)
    for name in archive.options: entries.append((data.tell(), 0, 0, 0, 'o', name))
    toc_offset = data.tell()
    for position, length, raw_length, compressed, kind, name in entries:
        encoded = name.encode('utf-8') + b'\0'
        size = (archive._TOC_ENTRY_LENGTH + len(encoded) + 15) // 16 * 16
        data.write(struct.pack(archive._TOC_ENTRY_FORMAT, size, position, length, raw_length, compressed, kind.encode()))
        data.write(encoded.ljust(size-archive._TOC_ENTRY_LENGTH, b'\0'))
    toc_length = data.tell()-toc_offset
    data.write(struct.pack(archive._COOKIE_FORMAT, cookie[0], data.tell()+archive._COOKIE_LENGTH, toc_offset, toc_length, cookie[4], cookie[5]))
    destination.mkdir(parents=True, exist_ok=True)
    output = destination / 'hongguo-ai-worker.exe'
    output.write_bytes(original[:archive._start_offset]+data.getvalue())
    manifest = {'version':'0.3.4','baseSha256':hashlib.sha256(original).hexdigest(),
                'workerSha256':hashlib.sha256(output.read_bytes()).hexdigest(),'modules':sorted(replaced)}
    (destination/'patch.json').write_text(json.dumps(manifest,indent=2),encoding='utf-8')
    print(json.dumps(manifest,indent=2))


if __name__ == '__main__':
    parser=argparse.ArgumentParser(); parser.add_argument('base',type=Path); parser.add_argument('destination',type=Path)
    args=parser.parse_args(); repack(args.base,args.destination)
