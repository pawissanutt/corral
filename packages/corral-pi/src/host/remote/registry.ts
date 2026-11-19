import { isBuiltinCommand } from "../../bridge/protocol.ts";

export interface RemoteMethod<Params> {
  readonly name: string;
  /** Validate wire arguments before any host action; throw BridgeRefusal for invalid input. */
  readonly parse: (args: Record<string, unknown>) => Params;
  readonly run: (params: Params) => object | Promise<object>;
}

export type RemoteHandler = (args: Record<string, unknown>) => object | Promise<object>;

export class RemoteRegistry {
  private readonly methods = new Map<string, RemoteHandler>();

  register<Params>(method: RemoteMethod<Params>): void {
    const { name } = method;
    if (isBuiltinCommand(name) || this.methods.has(name)) throw new Error(`duplicate session socket command: ${name}`);
    this.methods.set(name, args => method.run(method.parse(args)));
  }

  get(name: string): RemoteHandler | undefined { return this.methods.get(name); }
  names(): readonly string[] { return [...this.methods.keys()]; }
}
