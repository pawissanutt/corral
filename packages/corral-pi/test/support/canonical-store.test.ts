import { expect, test } from "bun:test";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { join } from "node:path";
import { StoreEventFollower, storeEvents } from "./canonical-store.ts";
import { corralBinary, ok, privateHerdr, privateRoot } from "./workspace.ts";

test("canonical follower subscribes before commits, retains every sequence and needs no legacy mirror", async () => {
  if (!privateHerdr()) { console.log("skipped: requires private herdr"); return; }
  const root=mkdtempSync(join(privateRoot()!,"canonical-events-"));
  const corral=corralBinary();
  const cli=(...args:string[])=>ok([corral,"--root",root,...args],{cwd:root});
  let follower:StoreEventFollower|undefined;
  try {
    cli("init");
    cli("new","Historical event","--idea");
    follower=await StoreEventFollower.open(root);
    const captured=follower.cursor;
    expect(captured).toBeGreaterThan(0);
    const first=follower.next(e=>e.task==="T-002");
    // Synchronous writers block this JS process, not the actual store observer process.
    cli("new","Canonical events","--idea");
    await first;
    cli("comment","T-002","same body");
    cli("comment","T-002","same body");
    const expected=storeEvents(corral,root).filter(e=>e.seq>captured);
    expect(expected.length).toBeGreaterThanOrEqual(3);
    for(const event of expected) {
      const observed=await follower.next(e=>e.seq===event.seq);
      expect(observed).toEqual(event);
    }
    expect(follower.malformed).toBe(0);
    expect(existsSync(join(root,".corral/events.jsonl"))).toBeFalse();
  } finally {follower?.close();rmSync(root,{recursive:true,force:true});}
},180_000);
