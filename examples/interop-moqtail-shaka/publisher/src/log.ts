const statusEl = document.getElementById("status") as HTMLDivElement;
const MAX_LOG_LINES = 20;
const logLines: string[] = [];

export function log(msg: string) {
	console.log(msg);
	logLines.push(msg);
	if (logLines.length > MAX_LOG_LINES) {
		logLines.shift();
	}
	statusEl.textContent = logLines.join("\n");
	statusEl.scrollTop = statusEl.scrollHeight;
}
