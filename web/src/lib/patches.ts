import catalog from '../generated/patch-catalog.json'

type Package = (typeof catalog.packages)[number]

export interface PatchInfo {
  id: string
  models: readonly ('L6' | 'L6max')[]
  title: string
  description: string
  readmeUrl: string
  demoImage?: string
  requestCommand?: string
}

export interface LoadablePatchInfo {
  id: string
  models: readonly ('L6' | 'L6max')[]
  title: string
  description: string
  /** Versioned filename hosted under `public/loadable-patches/<model>/`. */
  fileName: string
  order: number
}

const readmeUrl = (patch: Package) =>
  `https://github.com/derekstavis/zoom-l6max/blob/main/patches/${patch.name}/${patch.readme}`
const models = (values: string[]) => values as ('L6' | 'L6max')[]

const visiblePackages = catalog.packages
  .filter((patch) => patch.website)
  .toSorted((left, right) => left.website_order - right.website_order)

export const PATCHES: PatchInfo[] = visiblePackages.map((patch) => ({
  id: patch.name,
  models: models(patch.models),
  title: patch.title,
  description: patch.description,
  readmeUrl: readmeUrl(patch),
  ...(patch.demo_image ? { demoImage: patch.demo_image } : {}),
  ...(patch.request_command ? { requestCommand: patch.request_command } : {}),
}))

/** Packages with published SD binaries for the website's installation flow. */
export const LOADABLE_PATCHES: LoadablePatchInfo[] = visiblePackages
  .filter((patch) => patch.website_sd_models.length > 0)
  .map((patch) => ({
    id: patch.name,
    models: models(patch.website_sd_models),
    title: patch.title,
    description: patch.description,
    fileName: patch.file_name,
    order: patch.order,
  }))
