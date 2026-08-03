import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { StrictMode } from "react"
import { createRoot } from "react-dom/client"
import { BrowserRouter } from "react-router-dom"

import "kiln/tokens"
import "kiln/tokens/fonts.css"
import "./styles.css"
import App from "./App"
import { AppToastProvider } from "./components/ui"

const queryClient = new QueryClient({
  defaultOptions: {
    queries: { staleTime: 30_000, refetchOnWindowFocus: false },
    mutations: { retry: false },
  },
})

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <AppToastProvider>
        <BrowserRouter>
          <App />
        </BrowserRouter>
      </AppToastProvider>
    </QueryClientProvider>
  </StrictMode>,
)
