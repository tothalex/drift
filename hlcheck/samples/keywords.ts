import { A } from "./a";
import type { B } from "./b";
export * as ns from "./c";
declare const g: unknown;
namespace Outer { export const x = 1; }
abstract class Base<T extends object = {}> implements Iface {
    public readonly a: string = "s";
    protected static b: number = 0;
    private c?: boolean;
    override d: any;
    constructor() { super(); }
    abstract m(): void;
    get value(): symbol { return Symbol("v"); }
    set value(v: symbol) {}
}
interface Iface { m(): void; }
enum Color { Red, Green }
type Alias<T> = T extends infer U ? keyof U : never;
function* gen(): Generator<number> { yield 1; }
async function run(): Promise<bigint> {
    let x: string | null = null;
    var y: undefined = undefined;
    const z = await Promise.resolve(10n);
    if (typeof x === "string" && x instanceof String) {}
    else if (x satisfies string | null) {}
    for (const k in { a: 1 }) { continue; }
    for (const v of [1, 2]) { break; }
    while (false) {}
    do {} while (false);
    switch (z) { case 10n: break; default: break; }
    try { throw new Error("e"); } catch (e) {} finally {}
    delete ({ a: 1 } as any).a;
    const t = this ?? null;
    const w = new.target;
    debugger;
    return z;
}
const check = (p: unknown): p is B => true;
const assertFn: (v: unknown) => asserts v is string = (v) => {};
