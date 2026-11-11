/// Whether `a` and `b` name the same codec and profile and differ at most in
/// their level, which a decoder reads from the bitstream: the renditions of a
/// video are encoded at levels that fit their sizes.
export function differsOnlyInLevel(a: string, b: string): boolean {
  const [familyA, ...fieldsA] = a.split('.')
  const [familyB, ...fieldsB] = b.split('.')
  if (familyA !== familyB || fieldsA.length !== fieldsB.length) {
    return false
  }
  switch (familyA) {
    case 'avc1':
    case 'avc3':
      // `PPCCLL`: profile, constraint flags, then level.
      return fieldsA.length === 1 && fieldsA[0].slice(0, 4) === fieldsB[0].slice(0, 4)
    case 'hvc1':
    case 'hev1':
      // The third field is the tier and the level, such as `L93`.
      return (
        fieldsA[2]?.[0] === fieldsB[2]?.[0] && fieldsA.every((field, index) => index === 2 || field === fieldsB[index])
      )
    case 'vp09':
    case 'av01':
      // The second field is the level (and, for AV1, the tier).
      return fieldsA.every((field, index) => index === 1 || field === fieldsB[index])
    default:
      return false
  }
}
