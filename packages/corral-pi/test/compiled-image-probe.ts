import { createReadTool } from "../node_modules/@earendil-works/pi-coding-agent/dist/core/tools/read.js";
const result = await createReadTool(process.cwd()).execute("read", { path: process.argv[2] ?? "" });
const image = result.content.find(block => block.type === "image");
if (!image) { console.error(JSON.stringify(result.content)); process.exit(1); }
console.log(`Pi read returned image part: ${image.mimeType}, ${image.data.length} base64 bytes`);
