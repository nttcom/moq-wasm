// Prints the CHANGELOG.md section of the version given, the body of its
// GitHub Release; fails when the changelog has no such section.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'

const [version] = process.argv.slice(2)
const changelog = readFileSync(resolve(import.meta.dirname, '../CHANGELOG.md'), 'utf8')
const sections = changelog.split(/^## /m).slice(1)
const section = sections.find((text) => text.startsWith(`${version}\n`))
if (!section) {
  console.error(`CHANGELOG.md has no section for ${version}`)
  process.exit(1)
}
process.stdout.write(section.slice(section.indexOf('\n') + 1).trim() + '\n')
