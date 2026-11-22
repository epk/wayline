import type { Column } from '../types.ts';

type Inflate = (bytes: Uint8Array, out: Uint8Array) => Uint8Array;

interface Field {
  name: string;
  kind: 'i32' | 'u32' | 'i64' | 'f64';
}

const MESSAGE_SCHEMA = 1;
const MESSAGE_RECORD_BATCH = 3;
const TYPE_INT = 2;
const TYPE_FLOAT = 3;
const CODEC_ZSTD = 1;

class Flat {
  readonly view: DataView;
  readonly bytes: Uint8Array;
  constructor(bytes: Uint8Array) {
    this.bytes = bytes;
    this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }
  root(): number {
    return this.view.getUint32(0, true);
  }
  field(table: number, index: number): number {
    const vtable = table - this.view.getInt32(table, true);
    const size = this.view.getUint16(vtable, true);
    const at = 4 + index * 2;
    if (at >= size) return 0;
    const offset = this.view.getUint16(vtable + at, true);
    return offset ? table + offset : 0;
  }
  ref(table: number, index: number): number {
    const at = this.field(table, index);
    return at ? at + this.view.getUint32(at, true) : 0;
  }
  u8(table: number, index: number, fallback = 0): number {
    const at = this.field(table, index);
    return at ? this.view.getUint8(at) : fallback;
  }
  i16(table: number, index: number, fallback = 0): number {
    const at = this.field(table, index);
    return at ? this.view.getInt16(at, true) : fallback;
  }
  i32(table: number, index: number, fallback = 0): number {
    const at = this.field(table, index);
    return at ? this.view.getInt32(at, true) : fallback;
  }
  i64(at: number): number {
    return Number(this.view.getBigInt64(at, true));
  }
  vector(table: number, index: number): { start: number; length: number } {
    const at = this.ref(table, index);
    return at ? { start: at + 4, length: this.view.getUint32(at, true) } : { start: 0, length: 0 };
  }
  string(table: number, index: number): string {
    const at = this.ref(table, index);
    if (!at) return '';
    const length = this.view.getUint32(at, true);
    return new TextDecoder().decode(this.bytes.subarray(at + 4, at + 4 + length));
  }
}

const schemaFields = (flat: Flat, schema: number): Field[] => {
  const list = flat.vector(schema, 1);
  const fields: Field[] = [];
  for (let i = 0; i < list.length; i++) {
    const slot = list.start + i * 4;
    const field = slot + flat.view.getUint32(slot, true);
    const name = flat.string(field, 0);
    const type = flat.u8(field, 2);
    const spec = flat.ref(field, 3);
    let kind: Field['kind'] | null = null;
    if (type === TYPE_INT) {
      const bits = flat.i32(spec, 0);
      const signed = flat.u8(spec, 1) === 1;
      if (bits === 32) kind = signed ? 'i32' : 'u32';
      else if (bits === 64 && signed) kind = 'i64';
    } else if (type === TYPE_FLOAT && flat.i16(spec, 0) === 2) kind = 'f64';
    if (!kind) throw Error('History column ' + name + ' has an unsupported Arrow type.');
    fields.push({ name, kind });
  }
  return fields;
};

const width = { i32: 4, u32: 4, i64: 8, f64: 8 } as const;

function values(kind: Field['kind'], data: Uint8Array, count: number): ArrayLike<number> {
  const copy = data.slice(0, count * width[kind]).buffer;
  if (kind === 'i32') return new Int32Array(copy, 0, count);
  if (kind === 'u32') return new Uint32Array(copy, 0, count);
  if (kind === 'f64') return new Float64Array(copy, 0, count);
  const big = new BigInt64Array(copy, 0, count);
  const out = new Float64Array(count);
  for (let i = 0; i < count; i++) out[i] = Number(big[i]);
  return out;
}

export interface IpcTable {
  rows: number;
  columns: Map<string, Column>;
}

export function readIpc(bytes: Uint8Array, inflate: Inflate): IpcTable {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let fields: Field[] | null = null;
  const parts: { count: number; values: ArrayLike<number>; valid: Uint8Array | null }[][] = [];
  let rows = 0;
  let at = 0;
  while (at + 4 <= bytes.length) {
    let size = view.getInt32(at, true);
    at += 4;
    if (size === -1) {
      if (at + 4 > bytes.length) break;
      size = view.getInt32(at, true);
      at += 4;
    }
    if (size === 0) break;
    const flat = new Flat(bytes.subarray(at, at + size));
    at += size;
    const message = flat.root();
    const header = flat.ref(message, 2);
    const kind = flat.u8(message, 1);
    const bodyAt = flat.field(message, 3);
    const bodyLength = bodyAt ? flat.i64(bodyAt) : 0;
    const body = bytes.subarray(at, at + bodyLength);
    at += bodyLength;
    if (kind === MESSAGE_SCHEMA) {
      fields = schemaFields(flat, header);
      continue;
    }
    if (kind !== MESSAGE_RECORD_BATCH) throw Error('History holds an unsupported Arrow message.');
    if (!fields) throw Error('History has a record batch before its schema.');
    const count = flat.i64(flat.field(header, 0));
    const nodes = flat.vector(header, 1);
    const buffers = flat.vector(header, 2);
    const compression = flat.ref(header, 3);
    if (compression && flat.u8(compression, 0) !== CODEC_ZSTD)
      throw Error('History uses an unsupported Arrow compression codec.');
    const buffer = (index: number): Uint8Array => {
      const slot = buffers.start + index * 16;
      const offset = flat.i64(slot);
      const length = flat.i64(slot + 8);
      const raw = body.subarray(offset, offset + length);
      if (!compression || !length) return raw;
      const plain = Number(new DataView(raw.buffer, raw.byteOffset, 8).getBigInt64(0, true));
      return plain === -1 ? raw.subarray(8) : inflate(raw.subarray(8), new Uint8Array(plain));
    };
    if (nodes.length !== fields.length || buffers.length !== fields.length * 2)
      throw Error('History record batch does not match its schema.');
    parts.push(
      fields.map((field, i) => {
        const nulls = flat.i64(nodes.start + i * 16 + 8);
        const valid = nulls ? buffer(i * 2) : null;
        return { count, values: values(field.kind, buffer(i * 2 + 1), count), valid };
      }),
    );
    rows += count;
  }
  if (!fields) throw Error('History has no Arrow schema.');
  const columns = new Map<string, Column>();
  fields.forEach((field, i) => {
    const pieces = parts.map((batch) => batch[i]);
    if (pieces.length === 1 && !pieces[0].valid) {
      columns.set(field.name, pieces[0].values);
      return;
    }
    const out: (number | null)[] = [];
    for (const piece of pieces)
      for (let r = 0; r < piece.count; r++)
        out.push(!piece.valid || (piece.valid[r >> 3] >> (r & 7)) & 1 ? piece.values[r] : null);
    columns.set(field.name, out);
  });
  return { rows, columns };
}
