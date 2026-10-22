"""Package already-built x64 runtime files; does not build, tag, or publish."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import struct
import zipfile


def pe_x64(data):
    if data[:2] != b'MZ' or len(data) < 64:
        raise ValueError('not a PE binary')
    offset = struct.unpack_from('<I', data, 60)[0]
    if data[offset:offset + 4] != b'PE\0\0':
        raise ValueError('invalid PE header')
    if struct.unpack_from('<H', data, offset + 4)[0] != 0x8664:
        raise ValueError('only Windows x64 is supported')
    sections = struct.unpack_from('<H', data, offset + 6)[0]
    optional = offset + 24
    optional_size = struct.unpack_from('<H', data, offset + 20)[0]
    if struct.unpack_from('<H', data, optional)[0] != 0x20b:
        raise ValueError('only PE32+ is supported')
    table = optional + optional_size

    def file_offset(rva):
        for index in range(sections):
            virtual_size, address, size, raw = struct.unpack_from('<IIII', data, table + 40*index + 8)
            if address <= rva < address + max(virtual_size, size):
                result = raw + rva - address
                if not raw <= result < min(raw + size, len(data)):
                    raise ValueError('import points outside section data')
                return result
        raise ValueError('unmapped import address')

    import_rva = struct.unpack_from('<I', data, optional + 120)[0]
    dependencies = []
    if import_rva:
        row = file_offset(import_rva)
        for _ in range(256):
            descriptor = struct.unpack_from('<IIIII', data, row)
            if not any(descriptor):
                break
            start = file_offset(descriptor[3])
            end = data.index(0, start, min(start + 256, len(data)))
            dependencies.append(data[start:end].decode('ascii'))
            row += 20
        else:
            raise ValueError('import descriptor limit exceeded')
    # Release builds link the CRT statically; a missing VC++ installation must
    # not stop the plugin or worker from loading before an error can be shown.
    if any(name.lower().startswith(('vcruntime', 'msvcp', 'concrt')) for name in dependencies):
        raise ValueError('release binary requires a separately installed VC++ runtime')
    return dependencies


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--release-dir', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    version = re.search(r'^version = "([0-9]+\.[0-9]+\.[0-9]+)"$',
                        (root / 'Cargo.toml').read_text(encoding='utf-8'), re.M).group(1)
    expected = f'Nexa-Blend-Importer-{version}-x64.zip'
    if args.output.name != expected:
        parser.error(f'output name must be {expected}')
    files = {}
    dependencies = {}
    for name in ('nexa_blend_importer_plugin.dll', 'nexa-blend-importer.exe'):
        path = args.release_dir / name
        if path.is_symlink() or not path.is_file():
            raise ValueError(f'missing regular binary: {name}')
        data = path.read_bytes()
        dependencies[name] = pe_x64(data)
        files[name] = data
    for name in ('README.md', 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'VERIFICATION.md', 'CHANGELOG.md', 'RELEASE_NOTES.md',
                 'licenses/Rust-standard-library.html'):
        files[name] = (root / name).read_bytes()
    manifest = {
        'product': 'Nexa Blend Importer', 'version': version,
        'platform': 'windows-x64', 'signed': False,
        'qualification': 'local M0-M5 fixture qualification; see VERIFICATION.md',
        'runtime_dependencies': dependencies,
        'files': {name: {'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()}
                  for name, data in files.items()},
    }
    files['manifest.json'] = (json.dumps(manifest, indent=2) + '\n').encode('utf-8')
    with zipfile.ZipFile(args.output, 'x', compression=zipfile.ZIP_DEFLATED) as archive:
        for name, data in files.items():
            archive.writestr('NexaBlendImporter/' + name, data)
    with zipfile.ZipFile(args.output) as archive:
        if archive.testzip() is not None or set(archive.namelist()) != {
                'NexaBlendImporter/' + name for name in files}:
            raise ValueError('archive integrity or entry list mismatch')
        for name, data in files.items():
            if archive.read('NexaBlendImporter/' + name) != data:
                raise ValueError(f'archive readback differs: {name}')
    digest = hashlib.sha256(args.output.read_bytes()).hexdigest()
    with args.output.with_suffix('.zip.sha256').open('x', encoding='ascii') as checksum:
        checksum.write(f'{digest}  {args.output.name}\n')
    print(json.dumps({'archive': str(args.output), 'bytes': args.output.stat().st_size,
                      'sha256': digest, 'files': len(files), 'readback': 'passed'}))


if __name__ == '__main__':
    main()
