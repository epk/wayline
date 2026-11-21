export const staleFiles = 'The page files changed. Reload the page.';

export class StaleFiles extends Error {
  constructor(detail: string) {
    super(staleFiles + ' (' + detail + ')');
    this.name = 'StaleFiles';
  }
}

const zstdMagic = [0x28, 0xb5, 0x2f, 0xfd];

type Fetch = (url: string) => Promise<Response>;

export async function fetchChunk(url: string, get: Fetch = fetch): Promise<Uint8Array> {
  let response: Response;
  try {
    response = await get(url);
  } catch (error) {
    throw new StaleFiles(url + ' could not be fetched: ' + String(error));
  }
  if (!response.ok) throw new StaleFiles('HTTP ' + response.status + ' for ' + url);
  if (response.redirected) throw new StaleFiles(url + ' redirected to ' + response.url);
  const bytes = new Uint8Array(await response.arrayBuffer());
  if (bytes.length < 4 || zstdMagic.some((byte, i) => bytes[i] !== byte))
    throw new StaleFiles(url + ' is not a zstd file');
  return bytes;
}
