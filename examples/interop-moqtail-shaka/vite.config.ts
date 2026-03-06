import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { defineConfig } from "vite";

const RELAY_CERT_PATH =
	process.env.RELAY_CERT_PATH ??
	resolve(__dirname, "../../relay/keys/cert.pem");

function computeCertHash(certPath: string): string {
	const pem = readFileSync(certPath, "utf-8");
	const b64 = pem
		.replace(/-----BEGIN CERTIFICATE-----/, "")
		.replace(/-----END CERTIFICATE-----/, "")
		.replace(/\s/g, "");
	const der = Buffer.from(b64, "base64");
	return createHash("sha256").update(der).digest("base64");
}

let certHashBase64: string;
try {
	certHashBase64 = computeCertHash(RELAY_CERT_PATH);
	console.log(`Cert hash (base64): ${certHashBase64}`);
} catch {
	console.warn(
		`Warning: Could not read cert at ${RELAY_CERT_PATH}. Run: npm run gen-cert`,
	);
	certHashBase64 = "";
}

// Shaka Player reads fingerprintUri as hex string.
// Convert base64 hash to hex for the /fingerprint.txt endpoint.
function base64ToHex(b64: string): string {
	return Buffer.from(b64, "base64").toString("hex");
}
const certHashHex = certHashBase64 ? base64ToHex(certHashBase64) : "";

export default defineConfig({
	define: {
		__CERT_HASH_BASE64__: JSON.stringify(certHashBase64),
	},
	plugins: [
		{
			name: "fingerprint-middleware",
			configureServer(server) {
				server.middlewares.use("/fingerprint.txt", (_req, res) => {
					res.setHeader("Content-Type", "text/plain");
					res.end(certHashHex);
				});
			},
		},
	],
	build: {
		rollupOptions: {
			input: {
				publisher: resolve(__dirname, "publisher/index.html"),
				subscriber: resolve(__dirname, "subscriber/index.html"),
			},
		},
	},
});
