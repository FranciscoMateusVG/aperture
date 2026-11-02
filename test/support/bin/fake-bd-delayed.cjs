#!/usr/bin/env node
// Real subprocess boundary: durable write + stdout precede a delayed exit.
const fs = require("node:fs");
const dir = process.env.BEADS_DIR;
const argv = process.argv.slice(2);
const op = argv[0];
const config = JSON.parse(fs.readFileSync(dir + "/behavior.json"));
const task = {id: "ap-delayed", title: "PAYLOAD-MUST-NOT-BE-LOGGED", status: "open", notes: ""};
const record = event => fs.appendFileSync(dir + "/events.jsonl", JSON.stringify({
  event, op, pid: process.pid, at: Date.now(), exportAuto: process.env.BD_EXPORT_AUTO ?? null
}) + "\n");
record("start");
if (config.holdStdoutMs && op === "create") {
  require("node:child_process").spawn(process.execPath,
    ["-e", "setTimeout(() => {}, " + config.holdStdoutMs + ")"],
    {stdio: ["ignore", "inherit", "ignore"]});
}
if (fs.existsSync(dir + "/active")) {
  try { process.kill(Number(fs.readFileSync(dir + "/active")), 0); record("OVERLAP"); } catch {}
}
fs.writeFileSync(dir + "/active", String(process.pid));
process.on("SIGTERM", () => {
  if (config.ignoreTerm) return;
  record("terminated"); fs.rmSync(dir + "/active", {force: true}); process.exit(143);
});
let value;
if (op === "create") {
  fs.writeFileSync(dir + "/task.json", JSON.stringify(task));
  value = task;
} else if (op === "update" || op === "close") {
  const t = fs.existsSync(dir + "/task.json") ? JSON.parse(fs.readFileSync(dir + "/task.json")) : task;
  if (op === "close") t.status = "closed";
  else {
    t.status = "in_progress";
    t.notes += argv.find(x => x.startsWith("--append-notes="))?.slice(15) ?? "";
  }
  fs.writeFileSync(dir + "/task.json", JSON.stringify(t)); value = [t];
} else if (op === "show") {
  value = [JSON.parse(fs.readFileSync(dir + "/task.json"))];
} else value = [{...task, status: "in_progress", assignee: "peppy"}];
if (config.malformedList && op === "list") value = [42];
if (!config.noOutput) process.stdout.write(JSON.stringify(value) + "\n");
const delay = config.delays?.[op] ?? 0;
setTimeout(() => {
  record("end"); fs.rmSync(dir + "/active", {force:true}); process.exit(0);
}, delay);
