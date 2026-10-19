import { useMemo, useRef, useState } from 'react'
import { AlertTriangle, CheckCircle2, ExternalLink, ShieldCheck, Upload } from 'lucide-react'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Checkbox } from '@/components/ui/checkbox'
import { Label } from '@/components/ui/label'
import { Separator } from '@/components/ui/separator'
import { applyPatches, type PatchOutcome } from '@/lib/wasm'
import { PATCHES } from '@/lib/patches'

type Status =
  | { kind: 'idle' }
  | { kind: 'working' }
  | { kind: 'error'; message: string }
  | { kind: 'done'; outcome: PatchOutcome; fileName: string }

function downloadName(originalName: string): string {
  const base = originalName.replace(/\.bin$/i, '') || 'L6max'
  return `${base}-patched.bin`
}

function triggerDownload(bytes: Uint8Array, name: string) {
  const blob = new Blob([bytes.slice().buffer], { type: 'application/octet-stream' })
  const url = URL.createObjectURL(blob)
  const link = document.createElement('a')
  link.href = url
  link.download = name
  link.click()
  URL.revokeObjectURL(url)
}

export default function App() {
  const [selected, setSelected] = useState<Set<string>>(
    () => new Set(PATCHES.map((patch) => patch.id)),
  )
  const [file, setFile] = useState<File | null>(null)
  const [status, setStatus] = useState<Status>({ kind: 'idle' })
  const fileInputRef = useRef<HTMLInputElement>(null)

  const canApply = useMemo(
    () => file !== null && selected.size > 0 && status.kind !== 'working',
    [file, selected, status],
  )

  function togglePatch(id: string, checked: boolean) {
    setSelected((previous) => {
      const next = new Set(previous)
      if (checked) next.add(id)
      else next.delete(id)
      return next
    })
  }

  async function handleApply() {
    if (!file) return
    setStatus({ kind: 'working' })
    try {
      const bytes = new Uint8Array(await file.arrayBuffer())
      const outcome = await applyPatches(bytes, [...selected])
      setStatus({ kind: 'done', outcome, fileName: file.name })
    } catch (error) {
      setStatus({
        kind: 'error',
        message: error instanceof Error ? error.message : String(error),
      })
    }
  }

  return (
    <div className="mx-auto flex min-h-svh max-w-2xl flex-col gap-8 px-4 py-10 sm:px-6">
      <header className="flex flex-col gap-3">
        <h1 className="text-3xl font-semibold tracking-tight">
          L6max firmware patcher
        </h1>
        <p className="text-muted-foreground text-sm leading-relaxed">
          Build a patched update package for the Zoom LiveTrak L6max, entirely
          in this browser tab. Pick the patches you want, upload your own
          firmware update file, and download the result — nothing is
          uploaded anywhere.{' '}
          <a
            className="underline underline-offset-3 hover:text-foreground"
            href="https://github.com/derekstavis/zoom-l6max"
            target="_blank"
            rel="noreferrer"
          >
            About this project
            <ExternalLink className="ml-0.5 inline size-3.5 align-text-top" />
          </a>
        </p>
      </header>

      <Alert>
        <ShieldCheck />
        <AlertTitle>Your firmware file never leaves this tab</AlertTitle>
        <AlertDescription>
          Everything — unpacking, patching, and repacking — runs locally in
          your browser via WebAssembly. No file is sent to any server.
        </AlertDescription>
      </Alert>

      <Card>
        <CardHeader>
          <CardTitle>1. Choose patches</CardTitle>
          <CardDescription>
            Each one is independent; combine any set. See each patch's own
            README for full write-up, limitations, and how it was verified.
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-5">
          {PATCHES.map((patch, index) => (
            <div key={patch.id}>
              {index > 0 && <Separator className="mb-5" />}
              <div className="flex items-start gap-3">
                <Checkbox
                  id={patch.id}
                  checked={selected.has(patch.id)}
                  onCheckedChange={(checked) => togglePatch(patch.id, checked === true)}
                  className="mt-0.5"
                />
                <div className="flex flex-col gap-1">
                  <Label htmlFor={patch.id} className="text-sm font-medium">
                    {patch.title}
                  </Label>
                  <p className="text-muted-foreground text-sm leading-relaxed">
                    {patch.description}
                  </p>
                  <a
                    className="text-muted-foreground w-fit text-xs underline underline-offset-3 hover:text-foreground"
                    href={patch.readmeUrl}
                    target="_blank"
                    rel="noreferrer"
                  >
                    Read the full write-up
                  </a>
                </div>
              </div>
            </div>
          ))}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>2. Upload your firmware update file</CardTitle>
          <CardDescription>
            The official Zoom update package (the file you'd otherwise copy to
            an SD card). Only the stock 1.10 MAIN image is supported.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <input
            ref={fileInputRef}
            type="file"
            accept=".bin"
            className="hidden"
            onChange={(event) => {
              setFile(event.target.files?.[0] ?? null)
              setStatus({ kind: 'idle' })
            }}
          />
          <button
            type="button"
            onClick={() => fileInputRef.current?.click()}
            className="border-input hover:bg-accent/50 flex w-full flex-col items-center gap-2 rounded-lg border border-dashed px-4 py-8 text-center transition-colors"
          >
            <Upload className="text-muted-foreground size-5" />
            <span className="text-sm font-medium">
              {file ? file.name : 'Click to choose a file'}
            </span>
            {file && (
              <span className="text-muted-foreground text-xs">
                {(file.size / 1024).toFixed(0)} KB
              </span>
            )}
          </button>
        </CardContent>
      </Card>

      <Button size="lg" disabled={!canApply} onClick={handleApply} className="w-full">
        {status.kind === 'working' ? 'Patching…' : 'Apply patches'}
      </Button>

      {status.kind === 'error' && (
        <Alert variant="destructive">
          <AlertTriangle />
          <AlertTitle>Couldn't patch this file</AlertTitle>
          <AlertDescription>{status.message}</AlertDescription>
        </Alert>
      )}

      {status.kind === 'done' && (
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <CheckCircle2 className="size-5 text-emerald-600 dark:text-emerald-400" />
              Patched successfully
            </CardTitle>
            <CardDescription>
              Version marker {status.outcome.report.version} · image{' '}
              {status.outcome.report.image_length.toLocaleString()} bytes
            </CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-4">
            <Button
              onClick={() =>
                triggerDownload(status.outcome.package, downloadName(status.fileName))
              }
            >
              Download {downloadName(status.fileName)}
            </Button>
            <div className="text-muted-foreground flex flex-col gap-1 text-xs">
              <span>Applied: {status.outcome.report.patches.map((p) => p.name).join(', ')}</span>
              <span className="break-all">
                Package SHA-256: {status.outcome.report.update_sha256}
              </span>
            </div>
            <Alert>
              <ShieldCheck />
              <AlertTitle>Before you install it</AlertTitle>
              <AlertDescription>
                {status.outcome.report.usage} Install the same way as any
                official update (SD card), and keep your original firmware
                file.
              </AlertDescription>
            </Alert>
          </CardContent>
        </Card>
      )}

      <footer className="text-muted-foreground mt-4 text-center text-xs">
        Not affiliated with or endorsed by Zoom Corporation. Experimental,
        independently reverse-engineered firmware patches — use at your own
        risk.
      </footer>
    </div>
  )
}
