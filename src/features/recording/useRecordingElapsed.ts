import { useEffect, useState } from "react";

export function useRecordingElapsed(startedAt: string | undefined, initialMs: number) {
  const [elapsed, setElapsed] = useState(initialMs);
  useEffect(() => {
    const receivedAt = Date.now();
    setElapsed(initialMs);
    const t = setInterval(() => setElapsed(initialMs + Date.now() - receivedAt), 250);
    return () => clearInterval(t);
  }, [startedAt, initialMs]);
  return elapsed;
}

