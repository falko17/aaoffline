import * as wasm from "aaoffline-wasm";

let result = await wasm.download();
const blob = new Blob([result]);

const url = URL.createObjectURL(blob);

const a = document.createElement('a');
a.href = url;
// TODO: If single file, should be HTML
a.download = 'case.zip';
a.click();
URL.revokeObjectURL(url);
