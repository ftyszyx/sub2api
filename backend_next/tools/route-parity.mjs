import fs from "node:fs";
import path from "node:path";

const root = path.resolve(import.meta.dirname, "..", "..");

const goRouteFiles = [
  "backend/internal/server/routes/auth.go",
  "backend/internal/server/routes/user.go",
  "backend/internal/server/routes/payment.go",
  "backend/internal/setup/handler.go",
  "backend/internal/server/routes/gateway.go",
  "backend/internal/server/routes/admin.go",
];

const rustRouteFile = "backend_next/crates/server/src/routes/mod.rs";
const outputFile = "backend_next/docs/RouteParity.md";

const adminCollections = [
  "users",
  "groups",
  "accounts",
  "proxies",
  "announcements",
  "redeem-codes",
  "promo-codes",
  "channels",
  "channel-monitors",
  "channel-monitor-templates",
  "scheduled-test-plans",
  "tls-fingerprint-profiles",
  "error-passthrough-rules",
  "user-attributes",
];

const topLevelRustPrefixes = [
  "/health",
  "/api/event_logging",
  "/setup",
  "/v1",
  "/v1beta",
  "/responses",
  "/backend-api",
  "/chat",
  "/embeddings",
  "/images",
  "/antigravity",
];

function readWorkspaceFile(file) {
  return fs.readFileSync(path.join(root, file), "utf8");
}

function stripLineComment(line) {
  const index = line.indexOf("//");
  return index === -1 ? line : line.slice(0, index);
}

function joinPath(base, suffix) {
  const joined = `${base || ""}${suffix || ""}`;
  return joined.replace(/\/+/g, "/").replace(/\/$/, "") || "/";
}

function routeKey(route) {
  return `${route.method} ${normalizePath(route.path)}`;
}

function normalizePath(input) {
  const normalized = input
    .replace(/\/+/g, "/")
    .replace(/\/$/, "")
    .replace(/:([A-Za-z_][A-Za-z0-9_]*)/g, ":param")
    .replace(/\*([A-Za-z_][A-Za-z0-9_]*)/g, "*wildcard") || "/";
  return normalized
    .replace("/v1beta/models/*wildcard", "/v1beta/models/:param")
    .replace("/antigravity/v1beta/models/*wildcard", "/antigravity/v1beta/models/:param");
}

function parseGoRoutes(file) {
  const routes = [];
  const scopes = [new Map()];
  const rootVar = file.includes("setup/handler.go") || file.endsWith("gateway.go") ? "r" : "v1";
  scopes[0].set(rootVar, file.includes("setup/handler.go") || file.endsWith("gateway.go") ? "" : "/api/v1");
  const lines = readWorkspaceFile(file).split(/\r?\n/);

  function currentScope() {
    return scopes[scopes.length - 1];
  }

  function lookup(name) {
    for (let index = scopes.length - 1; index >= 0; index -= 1) {
      if (scopes[index].has(name)) {
        return scopes[index].get(name);
      }
    }
    return undefined;
  }

  function define(name, value) {
    currentScope().set(name, value);
  }

  for (const rawLine of lines) {
    const line = stripLineComment(rawLine);
    for (const char of line) {
      if (char === "{") {
        scopes.push(new Map(currentScope()));
      } else if (char === "}" && scopes.length > 1) {
        scopes.pop();
      }
    }

    if (
      file.endsWith("admin.go") &&
      /\bfunc\s+register\w+Routes\(\s*admin\s+\*gin\.RouterGroup/.test(line)
    ) {
      define("admin", "/api/v1/admin");
    }

    const group = line.match(/\b(\w+)\s*:=\s*(\w+)\.Group\(\s*"([^"]*)"\s*\)/);
    if (group) {
      const parent = lookup(group[2]);
      if (parent !== undefined) {
        define(group[1], joinPath(parent, group[3]));
      }
    }

    const routePattern = /\b(\w+)\.(GET|POST|PUT|DELETE|PATCH|Any)\(\s*"([^"]*)"\s*,\s*([^)\n]+)/g;
    let match;
    while ((match = routePattern.exec(line)) !== null) {
      const base = lookup(match[1]);
      if (base === undefined) {
        continue;
      }
      const method = match[2].toUpperCase() === "ANY" ? "ANY" : match[2].toUpperCase();
      routes.push({
        method,
        path: joinPath(base, match[3]),
        handler: match[4].trim(),
        source: file,
      });
    }
  }
  return routes;
}

function findMatchingParen(text, openIndex) {
  let depth = 0;
  let quote = false;
  let escaped = false;
  for (let index = openIndex; index < text.length; index += 1) {
    const char = text[index];
    if (quote) {
      if (escaped) {
        escaped = false;
      } else if (char === "\\") {
        escaped = true;
      } else if (char === '"') {
        quote = false;
      }
      continue;
    }
    if (char === '"') {
      quote = true;
    } else if (char === "(") {
      depth += 1;
    } else if (char === ")") {
      depth -= 1;
      if (depth === 0) {
        return index;
      }
    }
  }
  return -1;
}

function splitTopLevelComma(input) {
  let depth = 0;
  let quote = false;
  let escaped = false;
  for (let index = 0; index < input.length; index += 1) {
    const char = input[index];
    if (quote) {
      if (escaped) {
        escaped = false;
      } else if (char === "\\") {
        escaped = true;
      } else if (char === '"') {
        quote = false;
      }
      continue;
    }
    if (char === '"') {
      quote = true;
    } else if (char === "(") {
      depth += 1;
    } else if (char === ")") {
      depth -= 1;
    } else if (char === "," && depth === 0) {
      return [input.slice(0, index), input.slice(index + 1)];
    }
  }
  return [input, ""];
}

function parseRustHandlerMethods(expr) {
  const methods = [];
  const pattern = /\b(get|post|put|delete|patch)\s*\(\s*([A-Za-z0-9_]+)/g;
  let match;
  while ((match = pattern.exec(expr)) !== null) {
    methods.push({
      method: match[1].toUpperCase(),
      handler: match[2],
    });
  }
  return methods;
}

function parseRustRoutes() {
  const text = readWorkspaceFile(rustRouteFile);
  const routes = [];
  let searchIndex = 0;
  while (true) {
    const routeIndex = text.indexOf(".route(", searchIndex);
    if (routeIndex === -1) {
      break;
    }
    const openIndex = text.indexOf("(", routeIndex);
    const closeIndex = findMatchingParen(text, openIndex);
    if (closeIndex === -1) {
      break;
    }
    const inner = text.slice(openIndex + 1, closeIndex);
    const [pathExpr, handlerExpr] = splitTopLevelComma(inner);
    const literal = pathExpr.match(/^\s*"([^"]+)"\s*$/);
    if (literal) {
      for (const route of parseRustHandlerMethods(handlerExpr)) {
        routes.push({
          ...route,
          path: rustFullPath(literal[1]),
          source: rustRouteFile,
        });
      }
    }
    searchIndex = closeIndex + 1;
  }

  for (const name of adminCollections) {
    const base = `/api/v1/admin/${name}`;
    routes.push({
      method: "GET",
      path: base,
      handler: "admin_list_collection",
      source: `${rustRouteFile}:admin_collection_routes`,
    });
    routes.push({
      method: "POST",
      path: base,
      handler: "admin_create_collection_item",
      source: `${rustRouteFile}:admin_collection_routes`,
    });
    for (const [method, handler] of [
      ["GET", "admin_get_collection_item"],
      ["PUT", "admin_update_collection_item"],
      ["DELETE", "admin_delete_collection_item"],
    ]) {
      routes.push({
        method,
        path: `${base}/:id`,
        handler,
        source: `${rustRouteFile}:admin_collection_routes`,
      });
    }
  }
  return routes;
}

function rustFullPath(routePath) {
  if (topLevelRustPrefixes.some((prefix) => routePath.startsWith(prefix))) {
    return routePath;
  }
  return `/api/v1${routePath}`;
}

function uniqueRoutes(routes) {
  const map = new Map();
  for (const route of routes) {
    const key = routeKey(route);
    if (!map.has(key)) {
      map.set(key, route);
    }
  }
  return [...map.values()].sort(compareRoute);
}

function compareRoute(left, right) {
  return left.path.localeCompare(right.path) || left.method.localeCompare(right.method);
}

function markdownTable(routes, columns) {
  if (routes.length === 0) {
    return "_None._\n";
  }
  const header = `| ${columns.join(" | ")} |`;
  const sep = `| ${columns.map(() => "---").join(" | ")} |`;
  const rows = routes.map((route) =>
    `| ${columns
      .map((column) => escapeCell(route[column] ?? ""))
      .join(" | ")} |`,
  );
  return `${header}\n${sep}\n${rows.join("\n")}\n`;
}

function escapeCell(value) {
  return String(value).replace(/\|/g, "\\|").replace(/\r?\n/g, " ");
}

const goRoutes = uniqueRoutes(goRouteFiles.flatMap(parseGoRoutes).filter((route) => route.method !== "ANY"));
const rustRoutes = uniqueRoutes(parseRustRoutes());
const goMap = new Map(goRoutes.map((route) => [routeKey(route), route]));
const rustMap = new Map(rustRoutes.map((route) => [routeKey(route), route]));
const missing = goRoutes.filter((route) => !rustMap.has(routeKey(route)));
const extra = rustRoutes.filter((route) => !goMap.has(routeKey(route)));
const matched = goRoutes.filter((route) => rustMap.has(routeKey(route)));

const now = new Date().toISOString();
const doc = `# Route Parity

Generated by \`node backend_next/tools/route-parity.mjs\` at \`${now}\`.

This inventory compares Go Gin route registration under \`backend/internal/server/routes\` plus setup routes with Rust Axum registration in \`backend_next/crates/server/src/routes/mod.rs\`. Dynamic Rust admin collection routes are expanded explicitly. It is a route-level checklist; it does not prove behavior parity.

## Summary

| Item | Count |
| --- | ---: |
| Go routes | ${goRoutes.length} |
| Rust routes | ${rustRoutes.length} |
| Matched routes | ${matched.length} |
| Missing in Rust | ${missing.length} |
| Extra in Rust | ${extra.length} |

## Missing In Rust

${markdownTable(missing, ["method", "path", "handler", "source"])}

## Rust Extra Routes

${markdownTable(extra, ["method", "path", "handler", "source"])}

## Matched Routes

${markdownTable(matched, ["method", "path", "handler", "source"])}
`;

fs.writeFileSync(path.join(root, outputFile), doc);
console.log(`wrote ${outputFile}`);
console.log(`go=${goRoutes.length} rust=${rustRoutes.length} matched=${matched.length} missing=${missing.length} extra=${extra.length}`);
