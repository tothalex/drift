#!/bin/sh
# Build the fixture repo the demo tapes record against: a small TS shop
# with committed history, working-tree edits, and an untracked file —
# one of everything drift shows — plus the branches and linked
# worktrees the review board lists. Idempotent; recreates
# /tmp/drift-demo.
set -e

DEMO=/tmp/drift-demo
rm -rf "$DEMO"
mkdir -p "$DEMO/src"
cd "$DEMO"
git init -q -b main
git config user.email demo@drift.dev
git config user.name "drift demo"
git config commit.gpgsign false

# Commit dates are set relative to now: the board's age column is the
# point of several rows, so "3m" and "1w" have to differ on screen.
# `git` takes no relative dates in the *_DATE variables, so they are
# epoch seconds counted back from now.
NOW=$(date +%s)
ago() { echo "@$((NOW - $1)) +0000"; } # ago <seconds>
MINUTE=60
DAY=86400

commit() { # commit <seconds ago> <message>
  when=$(ago "$1")
  GIT_AUTHOR_DATE="$when" GIT_COMMITTER_DATE="$when" git commit -qm "$2"
}

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

printf '.claude/\nnode_modules/\n' > .gitignore

git add -A
commit $((21 * DAY)) "initial shop cart"

# Three branches off main, two of them checked out in linked worktrees
# the way an agent working in parallel leaves them.
branch() { # branch <name> <dir> <seconds ago> <message> <file> <body>
  git branch -q "$1" main
  git worktree add -q --checkout "$2" "$1"
  printf '%s' "$6" > "$2/$5"
  git -C "$2" add -A
  when=$(ago "$3")
  GIT_AUTHOR_DATE="$when" GIT_COMMITTER_DATE="$when" git -C "$2" commit -qm "$4"
}

branch feature/receipt-lines .claude/worktrees/receipt-lines $((12 * MINUTE)) \
  "feat: itemize the receipt" src/receipt-lines.ts \
  'import { Item } from "./models";

export function lines(items: Item[]): string[] {
  return items.map((i) => `${i.name}  ${i.price.toFixed(2)}`);
}
'
# A second commit, and an edit left uncommitted in that worktree: the
# row reads +2 ~1, which is what the board is for.
WHEN=$(ago $((4 * MINUTE)))
GIT_AUTHOR_DATE="$WHEN" GIT_COMMITTER_DATE="$WHEN" \
  git -C .claude/worktrees/receipt-lines commit -q --allow-empty \
  -m "feat: widen the receipt header"
perl -0pi -e 's/i\.price\.toFixed\(2\)/i.price.toFixed(2).padStart(8)/' \
  .claude/worktrees/receipt-lines/src/receipt-lines.ts

branch fix/empty-cart .claude/worktrees/empty-cart $((4 * DAY)) \
  "fix: reject an empty cart earlier" src/guard.ts \
  'import { Cart } from "./cart";

export function assertFilled(cart: Cart): void {
  if (cart.count() === 0) {
    throw new Error("cart is empty");
  }
}
'

# A branch nothing has checked out — reviewable at its tip all the same.
git branch -q chore/bump-deps main
git worktree add -q --detach .claude/tmp-bump chore/bump-deps
printf '{\n  "typescript": "5.6.2"\n}\n' > .claude/tmp-bump/package.json
git -C .claude/tmp-bump add -A
WHEN=$(ago $((8 * DAY)))
GIT_AUTHOR_DATE="$WHEN" GIT_COMMITTER_DATE="$WHEN" \
  git -C .claude/tmp-bump commit -qm "chore: bump typescript to 5.6"
git -C .claude/tmp-bump branch -qf chore/bump-deps HEAD
git worktree remove --force .claude/tmp-bump

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
