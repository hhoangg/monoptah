import base from "../eslint.config.js";

export default [...base, { ignores: [".drizzle-out/**", "migrations/**"] }];
