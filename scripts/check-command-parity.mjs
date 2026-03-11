import fs from "node:fs";
import path from "node:path";

function readFile(...parts) {
  const filePath = path.join(process.cwd(), ...parts);
  return fs.readFileSync(filePath, "utf8");
}

function listFilesRecursive(dirPath, files = []) {
  for (const entry of fs.readdirSync(dirPath, { withFileTypes: true })) {
    const fullPath = path.join(dirPath, entry.name);
    if (entry.isDirectory()) {
      listFilesRecursive(fullPath, files);
      continue;
    }
    files.push(fullPath);
  }
  return files;
}

function snakeToCamel(input) {
  return input.replace(/_([a-z])/g, (_, c) => c.toUpperCase());
}

function camelToSnake(input) {
  return input
    .replace(/([A-Z])/g, "_$1")
    .replace(/-/g, "_")
    .toLowerCase();
}

function formatList(items) {
  return items.map((item) => `  - ${item}`).join("\n");
}

function splitTopLevelByComma(input) {
  const parts = [];
  let current = "";
  let depthParen = 0;
  let depthBrace = 0;
  let depthBracket = 0;
  let depthAngle = 0;
  let stringQuote = null;
  let escaped = false;

  for (let i = 0; i < input.length; i += 1) {
    const ch = input[i];

    if (stringQuote) {
      current += ch;
      if (escaped) {
        escaped = false;
        continue;
      }
      if (ch === "\\") {
        escaped = true;
        continue;
      }
      if (ch === stringQuote) {
        stringQuote = null;
      }
      continue;
    }

    if (ch === '"' || ch === "'" || ch === "`") {
      stringQuote = ch;
      current += ch;
      continue;
    }

    if (ch === "(") depthParen += 1;
    if (ch === ")") depthParen = Math.max(0, depthParen - 1);
    if (ch === "{") depthBrace += 1;
    if (ch === "}") depthBrace = Math.max(0, depthBrace - 1);
    if (ch === "[") depthBracket += 1;
    if (ch === "]") depthBracket = Math.max(0, depthBracket - 1);
    if (ch === "<") depthAngle += 1;
    if (ch === ">") depthAngle = Math.max(0, depthAngle - 1);

    if (
      ch === "," &&
      depthParen === 0 &&
      depthBrace === 0 &&
      depthBracket === 0 &&
      depthAngle === 0
    ) {
      const part = current.trim();
      if (part) parts.push(part);
      current = "";
      continue;
    }

    current += ch;
  }

  const last = current.trim();
  if (last) parts.push(last);
  return parts;
}

function readBalancedBlock(source, openIndex, openChar = "{", closeChar = "}") {
  if (source[openIndex] !== openChar) {
    throw new Error(`Expected '${openChar}' at index ${openIndex}`);
  }

  let depth = 0;
  let stringQuote = null;
  let escaped = false;
  for (let i = openIndex; i < source.length; i += 1) {
    const ch = source[i];

    if (stringQuote) {
      if (escaped) {
        escaped = false;
        continue;
      }
      if (ch === "\\") {
        escaped = true;
        continue;
      }
      if (ch === stringQuote) {
        stringQuote = null;
      }
      continue;
    }

    if (ch === '"' || ch === "'" || ch === "`") {
      stringQuote = ch;
      continue;
    }

    if (ch === openChar) depth += 1;
    if (ch === closeChar) depth -= 1;

    if (depth === 0) {
      return {
        inner: source.slice(openIndex + 1, i),
        endIndex: i,
      };
    }
  }

  throw new Error("Unbalanced block parsing failure");
}

function collectBackendCommandsFromHandler(libSource) {
  const handlerMatch = libSource.match(/generate_handler!\s*\[([\s\S]*?)\]/m);
  if (!handlerMatch) {
    throw new Error("Could not find tauri::generate_handler![] block in src-tauri/src/lib.rs");
  }

  return new Set(
    handlerMatch[1]
      .split("\n")
      .map((line) => line.replace(/\/\/.*$/, "").trim())
      .map((line) => line.replace(/,$/, ""))
      .filter((line) => /^[a-zA-Z_][a-zA-Z0-9_]*$/.test(line)),
  );
}

function collectBackendCommandSignatures() {
  const commandsDir = path.join(process.cwd(), "src-tauri", "src", "commands");
  const files = listFilesRecursive(commandsDir).filter((filePath) => filePath.endsWith(".rs"));
  const signatures = new Map();

  const fnRegex =
    /#\s*\[\s*tauri::command\s*\][\s\S]*?(?:pub\s+async\s+fn|pub\s+fn)\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*\(([\s\S]*?)\)\s*->/g;

  for (const filePath of files) {
    const source = fs.readFileSync(filePath, "utf8");
    for (const match of source.matchAll(fnRegex)) {
      const commandName = match[1];
      const paramsBlock = match[2];
      const params = [];

      for (const part of splitTopLevelByComma(paramsBlock)) {
        const colonIndex = part.indexOf(":");
        if (colonIndex < 0) continue;

        const rawName = part.slice(0, colonIndex).trim().replace(/^mut\s+/, "");
        const typeText = part.slice(colonIndex + 1).trim();

        if (!/^[a-zA-Z_][a-zA-Z0-9_]*$/.test(rawName)) continue;
        if (
          typeText.includes("State<") ||
          typeText.includes("AppHandle") ||
          typeText.includes("tauri::Window") ||
          typeText.includes("Window<")
        ) {
          continue;
        }
        params.push(rawName);
      }

      signatures.set(commandName, params);
    }
  }

  return signatures;
}

function collectFrontendCommandMap(apiSource) {
  const commandsMatch = apiSource.match(/export const COMMANDS = \{([\s\S]*?)\}\s*as const;/m);
  if (!commandsMatch) {
    throw new Error("Could not find COMMANDS map in src/lib/tauri/api.ts");
  }

  const keyToCommand = new Map();
  const commandToKey = new Map();
  for (const match of commandsMatch[1].matchAll(/([a-zA-Z0-9_]+)\s*:\s*"([a-z0-9_]+)"\s*,?/g)) {
    const key = match[1];
    const command = match[2];
    keyToCommand.set(key, command);
    commandToKey.set(command, key);
  }

  return { keyToCommand, commandToKey };
}

function collectFrontendSpecArgs(apiSource) {
  const specStart = apiSource.indexOf("type CommandSpec = {");
  if (specStart < 0) {
    throw new Error("Could not find type CommandSpec in src/lib/tauri/api.ts");
  }

  const openBrace = apiSource.indexOf("{", specStart);
  const specBlock = readBalancedBlock(apiSource, openBrace);
  const body = specBlock.inner;

  const specArgs = new Map();
  const entryRegex = /\[COMMANDS\.([a-zA-Z0-9_]+)\]\s*:\s*\{/g;
  let entryMatch = entryRegex.exec(body);

  while (entryMatch) {
    const commandKey = entryMatch[1];
    const entryOpen = body.indexOf("{", entryMatch.index);
    const entryBlock = readBalancedBlock(body, entryOpen);
    const entryText = entryBlock.inner;

    let argKeys = [];
    if (!/args\?\s*:\s*undefined/.test(entryText)) {
      const argsMatch = entryText.match(/args\s*:\s*\{([\s\S]*?)\}\s*;/m);
      if (argsMatch) {
        argKeys = [...argsMatch[1].matchAll(/([a-zA-Z_][a-zA-Z0-9_]*)\??\s*:/g)].map(
          (m) => m[1],
        );
      }
    }

    specArgs.set(commandKey, new Set(argKeys));
    entryRegex.lastIndex = entryBlock.endIndex + 1;
    entryMatch = entryRegex.exec(body);
  }

  return specArgs;
}

function checkCommandNameParity(backendCommands, frontendCommandToKey) {
  const frontendCommands = new Set(frontendCommandToKey.keys());
  const missingInFrontend = [...backendCommands].filter((cmd) => !frontendCommands.has(cmd)).sort();
  const missingInBackend = [...frontendCommands].filter((cmd) => !backendCommands.has(cmd)).sort();
  return { missingInFrontend, missingInBackend };
}

function checkArgumentParity(backendCommands, backendSignatures, frontendCommandToKey, frontendSpecArgs) {
  const issues = [];

  for (const commandName of backendCommands) {
    const commandKey = frontendCommandToKey.get(commandName);
    if (!commandKey) continue;

    const backendArgs = backendSignatures.get(commandName) ?? [];
    const frontendArgs = frontendSpecArgs.get(commandKey);
    if (!frontendArgs) {
      issues.push(
        `${commandName}: missing [COMMANDS.${commandKey}] entry in CommandSpec for argument validation`,
      );
      continue;
    }

    for (const arg of backendArgs) {
      const variants = new Set([arg, snakeToCamel(arg), camelToSnake(arg)]);
      const matched = [...variants].some((name) => frontendArgs.has(name));
      if (!matched) {
        issues.push(
          `${commandName}: missing argument '${arg}' in frontend CommandSpec (accepted keys: ${[
            ...variants,
          ].join(", ")})`,
        );
      }
    }
  }

  return issues.sort();
}

const libSource = readFile("src-tauri", "src", "lib.rs");
const apiSource = readFile("src", "lib", "tauri", "api.ts");

const backendCommands = collectBackendCommandsFromHandler(libSource);
const backendSignatures = collectBackendCommandSignatures();
const frontendMap = collectFrontendCommandMap(apiSource);
const frontendSpecArgs = collectFrontendSpecArgs(apiSource);

const commandParity = checkCommandNameParity(backendCommands, frontendMap.commandToKey);
const argumentIssues = checkArgumentParity(
  backendCommands,
  backendSignatures,
  frontendMap.commandToKey,
  frontendSpecArgs,
);

if (
  commandParity.missingInFrontend.length > 0 ||
  commandParity.missingInBackend.length > 0 ||
  argumentIssues.length > 0
) {
  console.error("Command parity check failed.");

  if (commandParity.missingInFrontend.length > 0) {
    console.error("\nBackend commands missing in frontend COMMANDS:");
    console.error(formatList(commandParity.missingInFrontend));
  }

  if (commandParity.missingInBackend.length > 0) {
    console.error("\nFrontend COMMANDS missing in backend generate_handler:");
    console.error(formatList(commandParity.missingInBackend));
  }

  if (argumentIssues.length > 0) {
    console.error("\nCommand argument contract drift:");
    console.error(formatList(argumentIssues));
  }

  process.exit(1);
}

console.log(
  `Command parity check passed (${backendCommands.size} commands, arguments validated).`,
);
