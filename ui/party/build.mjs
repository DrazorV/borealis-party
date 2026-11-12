// Builds the party web app into ui/party/dist.
import { fileURLToPath, URL } from "node:url";
import { build } from "vite";

await build({
  root: fileURLToPath(new URL("..", import.meta.url)),
  configFile: fileURLToPath(new URL("./vite.config.ts", import.meta.url)),
});
