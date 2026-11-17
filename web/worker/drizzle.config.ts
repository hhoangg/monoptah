// Used only to generate SQL from src/db/schema.ts into a scratch directory:
//   pnpm --dir web --filter @monoptah/worker db:generate
// The committed migrations in ./migrations are numbered by hand (forward-only, workstreams.md 2.3);
// copy the generated SQL there as the next NNNN_name.sql without the statement-breakpoint comments.
import { defineConfig } from "drizzle-kit";

export default defineConfig({
  dialect: "sqlite",
  schema: "./src/db/schema.ts",
  out: "./.drizzle-out",
});
