// The pinned upstream resolver addresses this exact entrypoint in Bun compiled hosts.
// Kept separate from the host so QuickJS execution stays in its worker thread.
import "@earendil-works/pi-codemode/worker";
