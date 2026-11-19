// Side-effect module: main.tsx imports it FIRST so the policy exists before any other module runs.
import { installTrustedTypesPolicy } from "./trusted-types";

installTrustedTypesPolicy();
