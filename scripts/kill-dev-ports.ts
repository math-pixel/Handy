import { exec } from "child_process";
import { promisify } from "util";

const execAsync = promisify(exec);
const PORTS = [1420];

async function killPort(port: number) {
  if (process.platform === "win32") {
    try {
      const { stdout } = await execAsync(`netstat -ano`);
      const lines = stdout.split("\n").filter((l) => l.includes(`:${port} `));
      const pids = new Set<string>();
      for (const line of lines) {
        const match = line.trim().match(/(\d+)\s*$/);
        if (match && match[1] !== "0") pids.add(match[1]);
      }
      for (const pid of pids) {
        try {
          await execAsync(`taskkill /F /PID ${pid}`);
          console.log(`  killed PID ${pid} (was using port ${port})`);
        } catch {
          // process may have already exited
        }
      }
    } catch {
      // netstat failed or no process on port
    }
  } else {
    try {
      await execAsync(
        `lsof -ti:${port} | xargs kill -9 2>/dev/null; true`,
      );
    } catch {
      // no process on port
    }
  }
}

for (const port of PORTS) {
  await killPort(port);
}
console.log("✓ dev ports cleared");
