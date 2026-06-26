import { Toaster as Sonner } from "sonner"

function Toaster() {
  return (
    <Sonner
      theme="dark"
      position="bottom-right"
      toastOptions={{
        style: {
          background: "var(--bg-elevated)",
          border: "1px solid var(--border)",
          color: "var(--text-primary)",
        },
      }}
    />
  )
}

export { Toaster }
