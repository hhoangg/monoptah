import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { appendFile, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { test } from "node:test";
import { devAppPath, parseDevArgs, startDev } from "./dev-native.mjs";

test("forwards app options and rejects the retired runner flags", () => {
  assert.deepEqual(parseDevArgs([]), { args: [] });
  assert.deepEqual(parseDevArgs(["--monocode-data", "--cwd", "a project"]), {
    args: ["--monocode-data", "--cwd", "a project"],
  });
  assert.deepEqual(parseDevArgs(["--dev-data", "--cwd", "--watch", "--monocode-db", "--bundle"]), {
    args: ["--dev-data", "--cwd", "--watch", "--monocode-db", "--bundle"],
  });
  assert.deepEqual(parseDevArgs(["--", "--cwd", "a project"]), {
    args: ["--cwd", "a project"],
  });
  assert.deepEqual(parseDevArgs(["--detach", "--cwd", "a project"]), {
    args: ["--cwd", "a project"], detach: true,
  });
  assert.deepEqual(parseDevArgs(["--cwd", "--detach"]), { args: ["--cwd", "--detach"] });
  for (const arg of ["--watch", "--bundle"]) {
    assert.throws(() => parseDevArgs([arg]), /not supported.*rebuild and restart/);
  }
});

async function fixture(t, env = {}, options = {}) {
  const root = await realpath(await mkdtemp(join(tmpdir(), "monocode-dev-test-")));
  const messages = [];
  const children = [];
  const calls = [];
  let dev;
  // Register cleanup before setup so failed assertions never leave child processes.
  t.after(async () => {
    await dev?.close();
    for (const child of children) if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
    await rm(root, { recursive: true, force: true });
  });
  await mkdir(join(root, "crates/app/src"), { recursive: true });
  await writeFile(join(root, "crates/app/src/main.rs"), "initial");
  const helper = join(root, "fake-process.mjs");
  await writeFile(helper, `
    import { appendFileSync, existsSync } from "node:fs";
    import { join } from "node:path";
    const root = process.cwd();
    const event = (name) => appendFileSync(join(root, "events"), name + "\\n");
    if (process.argv[2] === "metadata") {
      console.log(JSON.stringify({ target_directory: join(root, "custom-target"), packages: [{name: "monocode-gpui", version: "0.15.0"}] }));
    } else if (process.argv[2] === "build") {
      event("build");
      await new Promise((resolve) => setTimeout(resolve, Number(process.env.TEST_BUILD_DELAY || 30)));
      if (existsSync(join(root, "fail-build"))) { event("failed"); process.exit(1); }
      event("built");
    } else if (process.argv[2] === "package") {
      event("package");
      if (existsSync(join(root, "fail-package"))) { event("package-failed"); process.exit(1); }
    } else {
      event("start:" + JSON.stringify(process.argv.slice(3)));
      process.stdin.once("data", () => { event("quit"); process.exit(0); });
      process.stdin.once("end", () => process.exit(0));
      process.stdin.resume();
    }
  `);
  dev = await startDev({
    root,
    platform: "linux",
    args: ["--dev-data", "--cwd", "a project with spaces"],
    debounce: 40,
    ...options,
    env: { ...process.env, ...env },
    log: (message) => messages.push(message),
    spawnProcess(command, args, options) {
      calls.push({ command, args, options });
      const task = command === "cargo" ? args[0] : command === "bash" ? "package" : "app";
      const child = spawn(process.execPath, [helper, task, ...(task === "app" ? args : [])], {
        ...options,
        stdio: command === "cargo" && args[0] === "metadata" ? ["ignore", "pipe", "pipe"] : ["pipe", "pipe", "pipe"],
      });
      children.push(child);
      return child;
    },
  });
  const events = async () => (await readFile(join(root, "events"), "utf8").catch(() => "")).trim().split("\n").filter(Boolean);
  const until = async (predicate) => {
    const deadline = Date.now() + 10_000;
    while (Date.now() < deadline) {
      if (await predicate(await events())) return;
      await delay(20);
    }
    assert.fail(`Timed out. Events: ${JSON.stringify(await events())}. Logs: ${messages.join("; ")}`);
  };
  const starts = (events) => events.filter((event) => event.startsWith("start:")).length;
  return { root, dev, calls, messages, events, until, starts };
}

test("file saves never rebuild, including edits during a requested build", { timeout: 20_000 }, async (t) => {
  const f = await fixture(t, { TEST_BUILD_DELAY: "250" });
  await f.until((events) => f.starts(events) === 1);
  for (let edit = 0; edit < 3; edit++) {
    await appendFile(join(f.root, "crates/app/src/main.rs"), `edit ${edit}`);
    await delay(75);
  }
  await delay(600);
  assert.equal((await f.events()).filter((event) => event === "build").length, 1);

  f.dev.requestBuild();
  await f.until((events) => events.filter((event) => event === "build").length === 2);
  await appendFile(join(f.root, "crates/app/src/main.rs"), "edit during manual build");
  await f.until((events) => f.starts(events) === 2);
  await delay(600);
  const events = await f.events();
  assert.equal(events.filter((event) => event === "build").length, 2);
  assert.equal(events.filter((event) => event === "quit").length, 1);
});

test("manual restart keeps the app on a failed build and shuts it down after a successful retry", { timeout: 20_000 }, async (t) => {
  const f = await fixture(t);
  await f.until((events) => f.starts(events) === 1);
  assert.deepEqual(f.calls.find((call) => call.args[0] === "build").args, ["build", "--manifest-path", "crates/monocode-gpui/Cargo.toml", "--locked", "-p", "monocode-gpui", "--bin", "monocode-gpui", "--profile", "native-dev"]);
  const launch = f.calls.find((call) => call.command !== "cargo");
  assert.ok(launch.command.startsWith(join(f.root, "custom-target")));
  assert.equal(launch.options.env.MONOCODE_DEV_RELOAD, "1");
  assert.deepEqual(launch.args, ["--dev-data", "--cwd", "a project with spaces"]);

  await writeFile(join(f.root, "fail-build"), "");
  f.dev.requestBuild();
  await f.until((events) => events.includes("failed"));
  assert.equal(f.starts(await f.events()), 1);
  assert.equal((await f.events()).includes("quit"), false);

  await rm(join(f.root, "fail-build"));
  f.dev.requestBuild();
  await f.until((events) => f.starts(events) === 2);
  const events = await f.events();
  assert.ok(events.lastIndexOf("built") < events.indexOf("quit"), events.join(", "));
  assert.ok(events.indexOf("quit") < events.findLastIndex((event) => event.startsWith("start:")));
  await f.dev.close();
  assert.equal((await f.events()).filter((event) => event === "quit").length, 2);
});

test("coalesces restart requests during a build without launching stale code or overlapping Cargo", { timeout: 20_000 }, async (t) => {
  const f = await fixture(t, { TEST_BUILD_DELAY: "900" });
  await f.until((events) => f.starts(events) === 1);
  f.dev.requestBuild();
  await f.until((events) => events.filter((event) => event === "build").length === 2);
  f.dev.requestBuild();
  await delay(100);
  f.dev.requestBuild();
  await f.until((events) => f.starts(events) === 2);
  const events = await f.events();
  assert.equal(events.filter((event) => event === "build").length, 3, events.join(", "));
  assert.equal(events.filter((event) => event === "quit").length, 1);
  assert.ok(events.indexOf("quit") > events.lastIndexOf("built"));
});

test("macOS bundles automatically and preserves the selected data mode for Finder", async (t) => {
  for (const [args, mode] of [
    [[], "dev-data"],
    [["--monocode-data"], "monocode-data"],
    [["--monocode-data", "--dev-data", "--cwd", "--monocode-data"], "dev-data"],
  ]) {
    await t.test(`${mode}: ${JSON.stringify(args)}`, async (t) => {
      const f = await fixture(t, { MONOCODE_DEV_APP_DIR: "/tmp/dev-apps" }, { platform: "darwin", args });
      await f.until((events) => f.starts(events) === 1);
      const packaged = f.calls.find((call) => call.command === "bash");
      assert.deepEqual(packaged.args, ["scripts/bundle-native-macos.sh", "native-dev", join(f.root, "custom-target"), "0.15.0", mode]);
      const app = f.calls.find((call) => !["cargo", "bash"].includes(call.command));
      assert.equal(app.command, join("/tmp/dev-apps", "Monoptah Native Dev.app/Contents/MacOS/monocode-gpui"));
      assert.deepEqual(app.args, args);
      const events = await f.events();
      assert.ok(events.indexOf("built") < events.indexOf("package"));
      assert.ok(events.indexOf("package") < events.findIndex((event) => event.startsWith("start:")));
    });
  }
});

test("failed macOS packaging keeps the current app running", async (t) => {
  const f = await fixture(t, {}, { platform: "darwin" });
  await f.until((events) => f.starts(events) === 1);
  await writeFile(join(f.root, "fail-package"), "");
  f.dev.requestBuild();
  await f.until((events) => events.includes("package-failed"));
  assert.equal(f.starts(await f.events()), 1);
  assert.equal((await f.events()).includes("quit"), false);
});

test("Windows launches the executable from the configured build target", async (t) => {
  const f = await fixture(t, { CARGO_BUILD_TARGET: "x86_64-pc-windows-msvc" }, { platform: "win32" });
  await f.until((events) => f.starts(events) === 1);
  assert.equal(f.calls.some((call) => call.command === "bash"), false);
  const app = f.calls.find((call) => call.command !== "cargo");
  assert.equal(app.command, join(f.root, "custom-target/x86_64-pc-windows-msvc/native-dev/monocode-gpui.exe"));
  assert.equal(app.options.detached, false);
});

test("closing during compilation terminates Cargo and never launches an app", { timeout: 10_000 }, async (t) => {
  const f = await fixture(t, { TEST_BUILD_DELAY: "5000" });
  await f.until((events) => events.includes("build"));
  await f.dev.close();
  assert.equal(f.starts(await f.events()), 0);
  assert.ok(f.calls.every((call) => call.command === "cargo"));
});

test("the dev bundle path is fixed and overridable", () => {
  assert.equal(devAppPath({ HOME: "/Users/a" }), join("/Users/a", "Applications", "Monoptah Native Dev.app"));
  assert.equal(devAppPath({ HOME: "/Users/a", MONOCODE_DEV_APP_DIR: "/x" }), join("/x", "Monoptah Native Dev.app"));
});

test("a detached verification app survives its agent launcher being reaped and keeps stdout and stderr", { timeout: 10_000 }, async (t) => {
  const root = await realpath(await mkdtemp(join(tmpdir(), "monoptah-detached-test-")));
  let appPid;
  let runner;
  t.after(async () => {
    runner?.kill("SIGKILL");
    if (appPid) {
      try { process.kill(appPid, "SIGTERM"); } catch {}
      // The app runs with the fixture as its cwd, and Windows refuses to remove a
      // directory a live process still holds, so wait for it to exit first.
      const running = () => {
        try { process.kill(appPid, 0); return true; } catch (error) {
          if (error.code === "ESRCH") return false;
          throw error;
        }
      };
      const deadline = Date.now() + 2000;
      while (running() && Date.now() < deadline) await delay(20);
      if (running()) {
        try { process.kill(appPid, "SIGKILL"); } catch {}
        while (running() && Date.now() < deadline + 2000) await delay(20);
      }
      assert.equal(running(), false, "cleanup must wait for the detached app to release its fixture");
    }
    // Windows can release file handles shortly after the process is gone.
    await rm(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  });
  const helper = join(root, "helper.mjs");
  await writeFile(helper, `
    import { writeFileSync } from "node:fs";
    import { join } from "node:path";
    const root = process.cwd();
    if (process.argv[2] === "metadata") {
      console.log(JSON.stringify({target_directory: root, packages: [{name: "monocode-gpui", version: "0.16.0"}]}));
    } else if (process.argv[2] === "app") {
      writeFileSync(join(root, "app.pid"), String(process.pid));
      console.log("verification stdout");
      console.error("verification stderr");
      process.on("SIGTERM", () => setTimeout(() => process.exit(0), 200));
      setInterval(() => writeFileSync(join(root, "heartbeat"), String(Date.now())), 20);
      if (process.env.MONOCODE_DEV_RELOAD === "1") {
        process.stdin.once("end", () => process.exit(0));
        process.stdin.resume();
      }
    }
  `);
  const modulePath = new URL("./dev-native.mjs", import.meta.url).href;
  const runnerPath = join(root, "runner.mjs");
  await writeFile(runnerPath, `
    import { spawn } from "node:child_process";
    import { appendFileSync } from "node:fs";
    import { startDev } from ${JSON.stringify(modulePath)};
    await startDev({root: ${JSON.stringify(root)}, platform: "darwin", detach: true,
      env: {...process.env, MONOCODE_DEV_RELOAD: "1", MONOCODE_DEV_LOG_DIR: ${JSON.stringify(join(root, "logs"))}},
      log: message => appendFileSync(${JSON.stringify(join(root, "runner.log"))}, message + String.fromCharCode(10)),
      spawnProcess(command, args, options) {
        const task = command === "cargo" ? args[0] : command === "bash" ? "package" : "app";
        return spawn(process.execPath, [${JSON.stringify(helper)}, task], options);
      }
    });
    setInterval(() => {}, 1000);
  `);
  runner = spawn(process.execPath, [runnerPath], { cwd: root, stdio: "ignore" });
  const deadline = Date.now() + 5000;
  while (Date.now() < deadline) {
    appPid = Number(await readFile(join(root, "app.pid"), "utf8").catch(() => "")) || undefined;
    if (appPid && await readFile(join(root, "heartbeat"), "utf8").catch(() => "")) break;
    await delay(20);
  }
  assert.ok(appPid, "the verification app launched");
  const closed = new Promise(resolve => runner.once("close", resolve));
  runner.kill("SIGKILL");
  await closed;
  await delay(150);
  const before = await readFile(join(root, "heartbeat"), "utf8");
  await delay(150);
  assert.notEqual(await readFile(join(root, "heartbeat"), "utf8"), before,
    "the app must continue running when its agent-owned launcher is reaped");
  const messages = await readFile(join(root, "runner.log"), "utf8");
  const logPath = messages.match(/Log: (.+)/)?.[1];
  assert.ok(logPath, messages);
  const output = await readFile(logPath, "utf8");
  assert.match(output, /verification stdout/);
  assert.match(output, /verification stderr/);
});
