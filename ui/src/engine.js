const [scriptUrl, wasmUrl, position, skill = 20] = await dioxus.recv();
const controller = new AbortController();
const urls = [];
let worker;
let timeout;
let rejectSearch;

const cancel = () => {
    controller.abort();
    worker?.terminate();
    rejectSearch?.(new Error("Search cancelled"));
};
dioxus.recv().then(cancel, cancel);

try {
    timeout = setTimeout(cancel, 15000);
    const fetchAsset = async (url, type) => {
        const response = await fetch(url, { signal: controller.signal });
        if (!response.ok) throw new Error("Could not load Stockfish");
        return type === "script" ? response.text() : response.arrayBuffer();
    };
    const [script, wasm] = await Promise.all([
        fetchAsset(scriptUrl, "script"), fetchAsset(wasmUrl, "wasm"),
    ]);
    controller.signal.throwIfAborted();
    const scriptBlob = URL.createObjectURL(new Blob([script], { type: "text/javascript" }));
    const wasmBlob = URL.createObjectURL(new Blob([wasm], { type: "application/wasm" }));
    urls.push(scriptBlob, wasmBlob);
    // Stockfish accepts its WASM location in the worker URL fragment.
    worker = new Worker(`${scriptBlob}#${encodeURIComponent(wasmBlob)}`);
    const bestMove = await new Promise((resolve, reject) => {
        rejectSearch = reject;
        worker.onerror = event => {
            event.preventDefault();
            reject(new Error("Stockfish could not run in this browser"));
        };
        worker.onmessage = ({ data }) => {
            if (data === "uciok") {
                worker.postMessage("setoption name Hash value 16");
                worker.postMessage(`setoption name Skill Level value ${Math.max(0, Math.min(20, skill))}`);
                worker.postMessage("isready");
            } else if (data === "readyok") {
                worker.postMessage(position);
                worker.postMessage("go movetime 750");
            } else if (typeof data === "string" && data.startsWith("bestmove ")) {
                resolve(data.split(/\s+/)[1]);
            }
        };
        worker.postMessage("uci");
    });
    dioxus.send({ best_move: bestMove, error: null });
} catch (error) {
    dioxus.send({ best_move: null, error: `Stockfish unavailable. ${error.message || "Please retry."}` });
} finally {
    clearTimeout(timeout);
    controller.abort();
    worker?.terminate();
    urls.forEach(url => URL.revokeObjectURL(url));
}
