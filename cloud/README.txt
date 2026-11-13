1. cloudflare.com -> Workers & Pages -> Create -> Worker, paste worker.js, Deploy.
2. Storage & Databases -> KV -> Create namespace "warden-sync".
3. Worker -> Settings -> Bindings -> Add -> KV namespace, variable name: SYNC, pick warden-sync.
4. Copy the worker URL (https://xxx.workers.dev) into the launcher: Settings -> Features -> Cloud account sync.
The first launcher that uploads a name sets its sync password. Use the same name+password on other computers.
