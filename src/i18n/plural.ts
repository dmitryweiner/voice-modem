export type PluralForms = Partial<Record<Intl.LDMLPluralRule, string>> & { other: string };

/** Picks the plural form of a word for n by the locale's rules (1 раз, 3 раза, 5 раз). */
export function plural(locale: string, n: number, forms: PluralForms): string {
  return forms[new Intl.PluralRules(locale).select(n)] ?? forms.other;
}

/** A whole number with its sign: +12, -4, 0. */
export function signed(value: number): string {
  const rounded = Math.round(value);
  return rounded > 0 ? `+${rounded}` : String(rounded);
}
