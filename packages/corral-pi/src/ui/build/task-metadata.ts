import type { BoardTaskMetadata } from "../../build/types.ts";

export function taskMetadataLabel({ size, difficulty, tags }: BoardTaskMetadata): string {
  return [size && `size: ${size}`, difficulty && `difficulty: ${difficulty}`,
    tags?.length && `tags: ${tags.join(", ")}`].filter(Boolean).join(" · ");
}
