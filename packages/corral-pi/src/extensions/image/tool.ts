import { createHash } from "node:crypto";
import type { ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import type { HostedRoute, ImageProvider } from "../../host/hosted-settings.ts";
import type { ModelUsage } from "../../tools/corral.ts";
import { generateImage, type ImageProviderOptions } from "./providers.ts";
import { preparePng, validatePng } from "./output.ts";

export interface ImageScope {
  /** No root means preview only. A child never inherits a parent's writable root implicitly. */
  readonly root?: string;
  refusal(path?: string): string | undefined;
  /** Authorize the prepared destination; return a check to repeat before publication. */
  authorize?(path: string, prompt: string, preparedPath: string, identity: string): () => void;
}

/** Shared by one host and its children: changing the tool binding cannot open another slot. */
export class ImageGeneration {
  private busy = false;
  private readonly stopped = new AbortController();
  constructor(private readonly route: HostedRoute<ImageProvider>, private readonly providers: ImageProviderOptions,
    private readonly report: (usage: ModelUsage) => void) {}

  close(): void { this.stopped.abort(); }

  tool(scope: ImageScope, platform = process.platform): ToolDefinition {
    const root = platform === "linux" ? scope.root : undefined;
    const { provider, model } = this.route;
    const billing = provider === "openai" ? "api" : "subscription";
    return { name: "image_gen", label: "Generate image",
      description: `Generate one PNG using ${provider}/${model} (${billing === "api" ? "metered image model" : "experimental subscription carrier; backend selects image model"}). `
        + (root ? "Requires an explicit new .png path (relative to the session or absolute); normal write authorization applies. Returns a preview and saves the PNG." : "Preview only. Do not pass a path."),
      parameters: Type.Object({ prompt: Type.String({ minLength: 1, maxLength: 8000 }), path: Type.Optional(Type.String({ minLength: 1, maxLength: 4096 })) }, { additionalProperties: false }),
      execute: async (_id, { prompt, path }: { prompt: string; path?: string }, caller) => {
        if (typeof prompt !== "string" || !prompt.trim() || prompt.length > 8000) throw new Error("image_generation_failed: provide a nonblank prompt of at most 8000 characters");
        const signal = AbortSignal.any([this.stopped.signal, AbortSignal.timeout(180000), ...(caller ? [caller] : [])]);
        const check = () => {
          signal.throwIfAborted();
          const refusal = scope.refusal(path);
          if (refusal) throw new Error(refusal);
          if (!root && path !== undefined) throw new Error("image_generation_unavailable: read-only preview cannot save a path");
          if (root && (!path || typeof path !== "string")) throw new Error("image_generation_unavailable: select a new .png path");
        };
        check();
        if (this.busy) throw new Error("image_generation_unavailable: another image request is running");
        this.busy = true;
        let output: ReturnType<typeof preparePng> | undefined;
        try {
          let recheck: (() => void) | undefined;
          if (root && path) {
            output = preparePng(root, path);
            if (!scope.authorize) throw new Error("image_generation_unavailable: write authorization is unavailable");
            recheck = scope.authorize(path, prompt, output.path, output.identity);
            check();
            output.revalidate();
            recheck?.();
          }
          const generated = await generateImage(this.providers, this.route, prompt, signal, () => {
            check();
            output?.revalidate();
            recheck?.();
          });
          const usage = generated.usage;
          // No image or hosted-tool fee can be inferred from a carrier's token price.
          this.report({ model: `${provider}/${model}`, billing, output: usage?.outputTokens,
            ...(usage?.cachedInputTokens === undefined ? {} : { cache_read: usage.cachedInputTokens }),
            ...(usage?.inputTokens !== undefined && usage.cachedInputTokens !== undefined && usage.inputTokens >= usage.cachedInputTokens
              ? { input: usage.inputTokens - usage.cachedInputTokens } : {}) });
          check();
          const bytes = validatePng(generated.base64);
          // Persist full resolution only at the requested path, not in long-lived model context.
          const preview = bytes.byteLength <= 96 * 1024 ? { mimeType: "image/png", data: generated.base64 }
            : await (await import("@earendil-works/pi-coding-agent")).resizeImage(bytes, "image/png", { maxBytes: 96 * 1024, maxWidth: 512, maxHeight: 512 });
          if (!preview || preview.data.length > 128 * 1024) throw new Error("image_generation_failed: cannot prepare a bounded preview");
          check();
          recheck?.();
          output?.publish(bytes);
          const receipt = { provider, model, ...(generated.servedModel ? { served_model: generated.servedModel } : {}), path: output?.path ?? null,
            bytes: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") };
          return { content: [{ type: "text", text: JSON.stringify(receipt) }, { type: "image", mimeType: preview.mimeType, data: preview.data }], details: undefined };
        } finally { try { output?.close(); } finally { this.busy = false; } }
      },
    };
  }
}
