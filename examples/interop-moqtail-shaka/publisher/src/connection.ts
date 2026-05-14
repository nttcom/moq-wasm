declare const __CERT_HASH_BASE64__: string;

import { MOQtailClient } from "moqtail";

function decodeCertHash(base64: string): Uint8Array {
	const binary = atob(base64);
	const bytes = new Uint8Array(binary.length);
	for (let i = 0; i < binary.length; i++) {
		bytes[i] = binary.charCodeAt(i);
	}
	return bytes;
}

export type MessageCallback = (msg: unknown) => void;

export async function createClient(opts?: {
	onMessageSent?: MessageCallback;
	onMessageReceived?: MessageCallback;
}): Promise<MOQtailClient> {
	const url = "https://localhost:4433";
	const certHash = decodeCertHash(__CERT_HASH_BASE64__);
	console.log("Connecting via WebTransport to", url);
	console.log("Cert hash:", __CERT_HASH_BASE64__);

	const client = await MOQtailClient.new({
		url,
		supportedVersions: [0xff00000e],
		enableDatagrams: false,
		transportOptions: {
			serverCertificateHashes: [
				{
					algorithm: "sha-256",
					value: certHash.buffer,
				},
			],
		},
		callbacks: {
			onSessionTerminated: (reason) => {
				console.warn("Session terminated:", reason);
			},
			onMessageSent: (msg) => {
				console.log(`[sent] ${msg.constructor.name}`, msg);
				opts?.onMessageSent?.(msg);
			},
			onMessageReceived: (msg) => {
				console.log(`[recv] ${msg.constructor.name}`, msg);
				opts?.onMessageReceived?.(msg);
			},
		},
	});

	return client;
}
