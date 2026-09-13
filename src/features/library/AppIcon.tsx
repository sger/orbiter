import { useState } from "react";
import { Box } from "lucide-react";

export function AppIcon({ src, name }: { src: string | null; name: string }) {
  const [failed, setFailed] = useState<string | null>(null);
  return (
    <div className="app-icon library-icon">
      {src && src !== failed ? (
        <img src={src} alt={`${name} icon`} onError={() => setFailed(src)} />
      ) : (
        <Box size={28} aria-label="App icon unavailable" />
      )}
    </div>
  );
}
