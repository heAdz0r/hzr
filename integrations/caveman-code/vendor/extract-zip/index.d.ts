interface Options {
  dir: string
  defaultDirMode?: number
  defaultFileMode?: number
  onEntry?: (entry: unknown, zipfile: unknown) => void
}

declare function extract(source: string, options: Options): Promise<void>

export = extract
