import { existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';

process.chdir(fileURLToPath(new URL('../', import.meta.url)));
if (existsSync('.env')) process.loadEnvFile('.env');
const localCargo = join(homedir(), '.cargo', 'bin', 'cargo');
const child = spawn(existsSync(localCargo) ? localCargo : 'cargo', ['run', '--release', '--bin', 'obleth-chat'], { stdio: 'inherit', env: process.env });
child.on('error', () => { console.error('Could not start Cargo. Install Rust using rustup, then retry.'); process.exitCode = 1; });
child.on('exit', (code, signal) => { process.exitCode = signal ? 1 : code ?? 1; });
for (const signal of ['SIGTERM', 'SIGINT']) process.on(signal, () => child.kill(signal));
