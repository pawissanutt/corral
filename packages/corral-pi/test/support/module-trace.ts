// A `bun --preload` plugin for tests at the compiled entry's boundary: it appends the path of every
// role or delivery module Bun loads to $CORRAL_MODULE_TRACE, then loads the module unchanged.
import { appendFileSync, readFileSync } from "node:fs";

interface PluginBuild {
  onLoad(options: { filter: RegExp }, callback: (args: { path: string }) => { contents: string; loader: "ts" }): void;
}
interface BunPlugins {
  plugin(plugin: { name: string; setup(build: PluginBuild): void }): void;
}

const trace = process.env.CORRAL_MODULE_TRACE;
const bun = Reflect.get(globalThis, "Bun") as BunPlugins;
bun.plugin({
  name: "corral-module-trace",
  setup(build) {
    build.onLoad({ filter: /\/src\/(roles\/(worker|reviewer|pm)|delivery)\/index\.ts$/ }, (args) => {
      if (trace) appendFileSync(trace, `${args.path}\n`);
      return { contents: readFileSync(args.path, "utf8"), loader: "ts" };
    });
  },
});
