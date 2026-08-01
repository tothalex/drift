#!/bin/sh
# Build the fixture repo the demo tapes record against: a small TS shop
# with committed history, working-tree edits, and an untracked file —
# one of everything drift shows. Idempotent; recreates /tmp/drift-demo.
set -e

DEMO=/tmp/drift-demo
rm -rf "$DEMO"
mkdir -p "$DEMO/src"
cd "$DEMO"
git init -q -b main

cat > src/cart.ts <<'EOF'
import { Item } from "./models";

export class Cart {
  items: Item[] = [];

  add(item: Item): void {
    this.items.push(item);
  }

  count(): number {
    return this.items.length;
  }

  total(): number {
    return this.items.map((i) => i.price * 1.27).reduce((a, b) => a + b, 0);
  }

  receipt(): string {
    const lines: string[] = [];
    const n = this.count();
    const sum = this.total();
    lines.push(`Subtotal: ${sum.toFixed(2)}`);
    lines.push(`Total: ${sum.toFixed(2)}`);
    return lines.join("\n");
  }
}
EOF

cat > src/models.ts <<'EOF'
export interface Item {
  name: string;
  price: number;
}

export interface Order {
  items: Item[];
  createdAt: Date;
}
EOF

cat > src/checkout.ts <<'EOF'
import { Cart } from "./cart";

export function checkout(cart: Cart): string {
  if (cart.count() === 0) {
    throw new Error("cart is empty");
  }
  return cart.receipt();
}
EOF

git add -A
git commit -qm "initial shop cart"

# Working-tree edits: the change the tapes walk through.
perl -0pi -e 's/lines\.push\(`Subtotal: \$\{sum\.toFixed\(2\)\}`\);/lines.push(`Subtotal (\${n} items): \${sum.toFixed(2)}`);/' src/cart.ts
perl -0pi -e 's/export interface Order \{\n  items: Item\[\];/export interface Order {\n  items: Item[];\n  discount?: number;/' src/models.ts

# An untracked file.
cat > src/receipt.ts <<'EOF'
export function header(shop: string): string {
  return `--- ${shop} ---`;
}
EOF

echo "fixture ready at $DEMO"
