"""Create an empty-pad emulator fixture from an existing MPC project.

The output is a separate file. Load it into the lab only after stopping ac50,
which otherwise saves over the fixture when stopped.
"""
import argparse
import gzip
import json
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    if args.source.resolve() == args.output.resolve():
        parser.error('source and output must be different files')
    raw = gzip.decompress(args.source.read_bytes())
    header = raw.split(b'\n', 5)
    if len(header) != 6 or header[0] != b'ACVS':
        parser.error('source is not an ACVS project')
    document = json.loads(header[5])
    data = document['data']
    data['samples'] = []
    for track in data['tracks']:
        if 'samples' in track:
            track['samples'] = []
        for instrument in track['program'].get('drum', {}).get('instruments', []):
            for layer in instrument.get('layersv', []):
                layer['sampleName'] = ''
                layer['sampleFile'] = ''
                layer['active'] = False
    content = b'\n'.join(header[:5]) + b'\n' + json.dumps(document).encode()
    args.output.write_bytes(gzip.compress(content, mtime=0))
    print(f'Wrote empty project: {args.output}')


if __name__ == '__main__':
    main()
