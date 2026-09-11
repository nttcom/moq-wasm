import { readFile } from "node:fs/promises";

export function parseApps(text) {
  const entries = JSON.parse(text);
  if (!Array.isArray(entries)) {
    throw new Error("apps file must be a JSON array");
  }
  const apps = new Map();
  entries.forEach((entry, index) => {
    const { appId, secret, isRelay } = entry ?? {};
    if (typeof appId !== "string" || appId === "") {
      throw new Error(`apps[${index}].appId must be a non-empty string`);
    }
    if (typeof secret !== "string" || secret === "") {
      throw new Error(`apps[${index}].secret must be a non-empty string`);
    }
    if (typeof isRelay !== "boolean") {
      throw new Error(`apps[${index}].isRelay must be a boolean`);
    }
    if (apps.has(appId)) {
      throw new Error(`apps[${index}].appId "${appId}" is duplicated`);
    }
    apps.set(appId, { secret, isRelay });
  });
  return apps;
}

export async function loadApps(path) {
  return parseApps(await readFile(path, "utf8"));
}
