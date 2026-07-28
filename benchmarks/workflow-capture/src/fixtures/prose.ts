/**
 * Compositional source material for the hybrid tasks.
 *
 * Each generator produces surface text plus the hidden values that text
 * encodes. The values are never written literally in a form a workflow could
 * copy: quantities appear in words, as sums of parts, or as ranges; dates
 * appear as fiscal and relative expressions; field labels and layouts are
 * redrawn every run. A workflow can only produce the hidden values by reading
 * the text, which is what puts a model on the runtime path.
 */

export interface Rng {
  int(min: number, maxInclusive: number): number;
  pick<T>(values: readonly T[]): T;
  sample<T>(values: readonly T[], count: number): T[];
  shuffle<T>(values: readonly T[]): T[];
  bool(): boolean;
}

const DAY_MS = 86_400_000;

export function isoDate(value: Date): string {
  return value.toISOString().slice(0, 10);
}

export function addDays(from: Date, days: number): Date {
  return new Date(from.getTime() + days * DAY_MS);
}

export function wholeDaysBetween(from: Date, to: Date): number {
  return Math.round(
    (Date.parse(`${isoDate(to)}T00:00:00Z`) - Date.parse(`${isoDate(from)}T00:00:00Z`)) / DAY_MS,
  );
}

/* ------------------------------------------------------------------ leads */

export interface SeatPhrase {
  seats: number;
  text: string;
}

/**
 * Every phrase carries at least one number that is *not* the seat count, so a
 * "first integer in the message" heuristic is wrong more often than it is
 * right.
 */
const SEAT_PHRASES: readonly SeatPhrase[] = [
  { seats: 120, text: "We are a hundred and twenty strong across our two offices, and we have been on the same tool for 3 years now." },
  { seats: 60, text: "Two teams of thirty would live in it day to day. There are another 400 people here who would never touch it." },
  { seats: 45, text: "Somewhere between thirty and forty-five of us would need logins, depending on how the pilot lands." },
  { seats: 8, text: "It would only be our small crew, eight of us. We did raise 12 million last year so we expect that to change." },
  { seats: 250, text: "Everyone at our Lyon site, so 250 people, plus around 30 contractors we would not be licensing." },
  { seats: 175, text: "Our support organisation is a hundred and seventy-five people. The wider group is closer to 900." },
  { seats: 95, text: "Ninety-five to begin with, with an option to double that in year two if it goes well." },
  { seats: 12, text: "A dozen of us, no more than that. We looked at 4 other vendors before writing to you." },
  { seats: 24, text: "Three squads of eight would be the initial rollout, out of 60 engineers overall." },
  { seats: 140, text: "Around 140 users. For reference our last vendor quoted 90000 euros for that number." },
  { seats: 30, text: "Nous serions une trentaine a l'utiliser, sur un effectif total de 210 personnes." },
  { seats: 500, text: "Wir haben 500 Mitarbeitende im Vertrieb, die damit arbeiten wuerden, dazu 1200 in der Produktion, die es nicht brauchen." },
  { seats: 70, text: "Seventy or so, spread over 5 countries." },
  { seats: 0, text: "We are honestly still working out who internally would end up using it." },
  { seats: 0, text: "Hard to put a number on it yet, it depends whether we go beyond the pilot at all." },
  { seats: 18, text: "Eighteen people in the first wave. Our head count is 230 but most are in manufacturing." },
];

export interface TimelinePhrase {
  days: number;
  text: string;
}

/** Fiscal, relative, and explicit-date expressions resolved against `asOf`. */
export function timelinePhrases(asOf: Date): readonly TimelinePhrase[] {
  const quarterEnd = endOfQuarter(asOf);
  const yearEnd = new Date(Date.UTC(asOf.getUTCFullYear(), 11, 31));
  const fiscalStart = new Date(Date.UTC(asOf.getUTCFullYear() + 1, 3, 1));
  const boardDemo = addDays(asOf, 33);
  const conference = addDays(asOf, 61);
  return [
    { days: 0, text: "We have no fixed date in mind at this stage." },
    { days: 0, text: "Kein konkreter Termin bisher, wir sondieren erst einmal den Markt." },
    { days: 10, text: "Our agreement with the incumbent lapses in ten days and we cannot be left without something." },
    { days: 21, text: "We would like to be running three weeks from now." },
    { days: 42, text: "Six weeks would be ideal for us." },
    { days: wholeDaysBetween(asOf, quarterEnd), text: "It needs to be in place before the current quarter closes." },
    { days: wholeDaysBetween(asOf, yearEnd), text: "Some time before the calendar year is out would suit us." },
    { days: wholeDaysBetween(asOf, fiscalStart), text: "We would target the start of our next financial year, which begins on 1 April." },
    { days: wholeDaysBetween(asOf, boardDemo), text: `We are presenting to our board on ${isoDate(boardDemo)} and would want to show it live.` },
    { days: wholeDaysBetween(asOf, conference), text: `Notre conference clients a lieu le ${isoDate(conference)} et nous aimerions etre operationnels avant.` },
    { days: 90, text: "Realistically about three months out." },
  ];
}

export const SECURITY_PHRASES: readonly string[] = [
  "Our security team will want to see your SOC 2 report before this goes any further.",
  "Legal will need to review the data processing terms with you.",
  "Procurement runs a full vendor assessment on anything at this size.",
  "Wir haben ein Informationssicherheits-Audit als festen Teil des Beschaffungsprozesses.",
  "Notre service juridique devra valider le contrat de sous-traitance avant signature.",
  "Anything touching customer records goes through our compliance group first.",
];

export const NO_SECURITY_PHRASES: readonly string[] = [
  "We can move quickly on our side, there is no heavy process to get through.",
  "I can sign this myself, so it is really just a question of fit.",
  "Wir entscheiden das im Team, ein langer Prozess steht dem nicht im Weg.",
  "Pas de procedure lourde chez nous, la decision se prend en interne rapidement.",
  "Budget is already set aside, so it comes down to whether the product does what we need.",
];

export const LEAD_COMPANIES: readonly string[] = [
  "Northwind Logistics", "Meridian Health", "Altmark Werke", "Brightpath Learning",
  "Lucania Foods", "Nordvik Marine", "Calvet Assurances", "Kaisei Robotics",
  "Ferngrove Estates", "Vespera Media", "Wisla Chemicals", "Auberive Energie",
  "Steinbach Logistik", "Puentes Construccion", "Larkmoor Financial", "Danubia Rail",
];

export const LEAD_SENDERS: readonly string[] = [
  "procurement@northwind.example", "t.reyes@meridianhealth.example",
  "einkauf@altmark.example", "hello@brightpath.example",
  "g.moretti@lucania.example", "post@nordvik.example",
  "achats@calvet.example", "s.tanaka@kaisei.example",
  "office@ferngrove.example", "m.duarte@vespera.example",
  "biuro@wisla.example", "contact@auberive.example",
  "info@steinbach.example", "compras@puentes.example",
  "ops@larkmoor.example", "kontakt@danubia.example",
];

const LEAD_OPENERS: readonly string[] = [
  "We came across you through a colleague and are looking at replacing what we currently run.",
  "Following up on the material your team sent over last month.",
  "We are running a short evaluation and would like to include you.",
  "Wir pruefen derzeit Alternativen zu unserem bestehenden System.",
  "Nous evaluons actuellement plusieurs solutions pour remplacer notre outil interne.",
  "Our current setup has outgrown itself and we are looking at what else is out there.",
];

const LEAD_CLOSERS: readonly string[] = [
  "Could someone walk us through it?",
  "Happy to set up a call next week.",
  "What would the next step look like?",
  "Ueber eine kurze Rueckmeldung wuerden wir uns freuen.",
  "Merci de nous indiquer la marche a suivre.",
];

export interface GeneratedLead {
  company: string;
  sender: string;
  subject: string;
  body: string;
  seatCount: number;
  timelineDays: number;
  securityReview: boolean;
}

/**
 * Draw `count` entries, preferring distinct ones. Banks smaller than the day's
 * volume fall back to repeats, which is what a real inbox looks like anyway.
 */
function draw<T>(random: Rng, values: readonly T[], count: number): T[] {
  const drawn = random.sample(values, count);
  while (drawn.length < count) drawn.push(random.pick(values));
  return drawn;
}

export function generateLeads(random: Rng, asOf: Date, count: number): GeneratedLead[] {
  const companies = draw(random, LEAD_COMPANIES, count);
  const senders = draw(random, LEAD_SENDERS, count);
  const seatChoices = draw(random, SEAT_PHRASES, count);
  const timelineChoices = draw(random, timelinePhrases(asOf), count);
  return companies.map((company, index) => {
    const seat = seatChoices[index]!;
    const timeline = timelineChoices[index]!;
    const securityReview = random.bool();
    const securityText = securityReview
      ? random.pick(SECURITY_PHRASES)
      : random.pick(NO_SECURITY_PHRASES);
    const middle = random.shuffle([seat.text, timeline.text, securityText]);
    return {
      company,
      sender: senders[index]!,
      subject: `Enquiry from ${company}`,
      body: [random.pick(LEAD_OPENERS), ...middle, random.pick(LEAD_CLOSERS)].join(" "),
      seatCount: seat.seats,
      timelineDays: timeline.days,
      securityReview,
    };
  });
}

export function leadScore(seatCount: number, timelineDays: number, securityReview: boolean): number {
  let score = 0;
  if (seatCount >= 100) score += 40;
  else if (seatCount >= 25) score += 25;
  else if (seatCount >= 1) score += 10;
  if (timelineDays >= 1 && timelineDays <= 45) score += 30;
  else if (timelineDays >= 46 && timelineDays <= 120) score += 15;
  if (!securityReview) score += 20;
  return score;
}

export function leadBand(score: number): "hot" | "warm" | "nurture" {
  if (score >= 70) return "hot";
  if (score >= 40) return "warm";
  return "nurture";
}

/* --------------------------------------------------------------- invoices */

export const INVOICE_VENDORS: readonly string[] = [
  "Halden Print Works", "Ostara Cloud Services", "Trellis Facilities",
  "Bramble & Coe Legal", "Kestrel Freight", "Ardent Analytics",
  "Rivermoor Catering", "Solvang Hardware", "Pentland Security",
  "Fabrik Weiss GmbH", "Atelier Verne", "Corvus Translations",
];

const NET_LABELS = ["Net", "Subtotal", "Amount before tax", "Net amount", "Zwischensumme", "Montant HT"];
const TAX_LABELS = ["VAT", "Tax", "Sales tax", "MwSt.", "TVA", "Tax charged"];
const GROSS_LABELS = ["Total", "Total due", "Amount payable", "Grand total", "Gesamtbetrag", "Total TTC"];
const DUE_LABELS = ["Due date", "Payment due", "Payable by", "Faelligkeitsdatum", "Echeance", "To be settled by"];
const NUMBER_LABELS = ["Invoice", "Invoice no.", "Document ref", "Rechnungsnummer", "Facture n", "Our ref"];

const CURRENCIES = [
  { code: "EUR", symbol: "EUR " },
  { code: "USD", symbol: "$" },
  { code: "GBP", symbol: "GBP " },
];

function formatInvoiceDate(value: Date, style: number): string {
  const iso = isoDate(value);
  const [year, month, day] = iso.split("-") as [string, string, string];
  const months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
  const longMonths = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
  const monthIndex = Number(month) - 1;
  if (style === 0) return iso;
  if (style === 1) return `${day}/${month}/${year}`;
  if (style === 2) return `${months[monthIndex]} ${Number(day)}, ${year}`;
  if (style === 3) return `${Number(day)} ${longMonths[monthIndex]} ${year}`;
  return `${year}.${month}.${day}`;
}

function formatAmount(value: number, style: number): string {
  const fixed = value.toFixed(2);
  if (style === 0) return fixed;
  const [whole, fraction] = fixed.split(".") as [string, string];
  const grouped = whole.replace(/\B(?=(\d{3})+(?!\d))/gu, style === 1 ? "," : ".");
  return style === 1 ? `${grouped}.${fraction}` : `${grouped},${fraction}`;
}

export interface GeneratedInvoice {
  title: string;
  text: string;
  vendor: string;
  invoiceNumber: string;
  currency: string;
  net: string;
  tax: string;
  gross: string;
  dueDate: string;
  blocked: boolean;
}

export function generateInvoices(
  random: Rng,
  asOf: Date,
  count: number,
  runTag: string,
): GeneratedInvoice[] {
  const vendors = random.sample(INVOICE_VENDORS, count);
  // Exactly one document per run fails its own arithmetic. Recording it as
  // written, rather than repairing it, is what the task scores.
  const blockedIndex = random.int(0, count - 1);
  return vendors.map((vendor, index) => {
    const currency = random.pick(CURRENCIES);
    const amountStyle = random.int(0, 2);
    const dateStyle = random.int(0, 4);
    const layout = random.int(0, 3);
    const net = random.int(40_000, 900_000) / 100;
    const taxRate = random.pick([0.05, 0.07, 0.19, 0.2, 0.21]);
    const tax = Math.round(net * taxRate * 100) / 100;
    const trueGross = Math.round((net + tax) * 100) / 100;
    const blocked = index === blockedIndex;
    const gross = blocked
      ? Math.round((trueGross + random.int(1_000, 9_000) / 100) * 100) / 100
      : trueGross;
    const dueDate = addDays(asOf, random.int(-25, 40));
    const invoiceNumber = `${vendor.slice(0, 3).toUpperCase()}-${random.int(1_000, 9_999)}`;
    const netLabel = random.pick(NET_LABELS);
    const taxLabel = random.pick(TAX_LABELS);
    const grossLabel = random.pick(GROSS_LABELS);
    const dueLabel = random.pick(DUE_LABELS);
    const numberLabel = random.pick(NUMBER_LABELS);
    const money = (value: number) => `${currency.symbol}${formatAmount(value, amountStyle)}`;
    const netLine = `${netLabel}: ${money(net)}`;
    const taxLine = `${taxLabel} (${Math.round(taxRate * 100)}%): ${money(tax)}`;
    const grossLine = `${grossLabel}: ${money(gross)}`;
    const dueLine = `${dueLabel}: ${formatInvoiceDate(dueDate, dateStyle)}`;
    const numberLine = `${numberLabel}: ${invoiceNumber}`;
    const header = [vendor, "Supplier statement", numberLine];
    const bodies = [
      [...header, dueLine, netLine, taxLine, grossLine],
      [...header, netLine, taxLine, grossLine, dueLine],
      [...header, taxLine, netLine, grossLine, dueLine],
      [vendor, numberLine, dueLine, "Services rendered as per agreement", grossLine, netLine, taxLine],
    ];
    return {
      title: `${vendor} ${invoiceNumber} ${runTag}`,
      text: [...bodies[layout]!, `Reference: ${runTag}`].join("\n"),
      vendor,
      invoiceNumber,
      currency: currency.code,
      net: net.toFixed(2),
      tax: tax.toFixed(2),
      gross: gross.toFixed(2),
      dueDate: isoDate(dueDate),
      blocked,
    };
  });
}

/* --------------------------------------------------------------- incident */

export const INCIDENT_COMPONENTS: readonly string[] = [
  "connection-pool", "cache-warmer", "rate-limiter", "config-loader",
  "retry-budget", "dns-resolver", "feature-flag", "batch-scheduler",
  "session-store", "queue-consumer",
];

export const INCIDENT_PEOPLE: readonly string[] = [
  "Priya", "Tomas", "Aisha", "Lars", "Mireille", "Kenji", "Rosa", "Dmitri",
];

const INCIDENT_CHATTER: readonly string[] = [
  "anyone else seeing the pager go off twice",
  "coffee run, back in five",
  "I am on the bridge if someone wants to join",
  "who is writing this one up afterwards",
  "customer support is asking what to tell people",
  "status page updated",
  "reminder that the deploy freeze is still on",
  "adding the on-call from the platform side",
];

export interface GeneratedIncident {
  text: string;
  confirmed: readonly { component: string; person: string }[];
  ruledOut: readonly string[];
}

export function generateIncident(random: Rng, runTag: string): GeneratedIncident {
  const confirmedCount = random.int(2, 4);
  const ruledOutCount = random.int(2, 3);
  const components = random.sample(INCIDENT_COMPONENTS, confirmedCount + ruledOutCount);
  const confirmedComponents = components.slice(0, confirmedCount);
  const ruledOut = components.slice(confirmedCount);
  const people = random.sample(INCIDENT_PEOPLE, Math.min(INCIDENT_PEOPLE.length, confirmedCount + 2));
  const confirmed = confirmedComponents.map((component, index) => ({
    component,
    person: people[index % people.length]!,
  }));
  const lines: string[] = [];
  for (const entry of confirmed) {
    lines.push(`[${randomClock(random)}] ${entry.person}: I think the ${entry.component} is involved, digging in`);
    lines.push(`[${randomClock(random)}] ${entry.person}: confirmed the ${entry.component} contributed to this, we reproduced it on the standby stack`);
  }
  for (const component of ruledOut) {
    const person = random.pick(people);
    lines.push(`[${randomClock(random)}] ${person}: could the ${component} be behind this?`);
    lines.push(`[${randomClock(random)}] ${person}: ruled out the ${component}, it was healthy throughout and we have the graphs to show it`);
  }
  for (const chatter of random.sample(INCIDENT_CHATTER, random.int(3, 6))) {
    lines.push(`[${randomClock(random)}] ${random.pick(people)}: ${chatter}`);
  }
  return {
    // The transcript is deliberately not in chronological order: the response
    // channel was not, and reading it correctly is part of the task.
    text: [`Incident channel export ${runTag}`, "", ...random.shuffle(lines)].join("\n"),
    confirmed,
    ruledOut,
  };
}

function randomClock(random: Rng): string {
  return `${String(random.int(6, 21)).padStart(2, "0")}:${String(random.int(0, 59)).padStart(2, "0")}`;
}

/* --------------------------------------------------------------- contract */

export interface GeneratedObligation {
  clause: string;
  party: string;
  obligation: string;
  noticeDays: number;
}

export interface GeneratedContract {
  text: string;
  termEnd: string;
  obligations: readonly GeneratedObligation[];
}

const OBLIGATION_TEMPLATES: readonly {
  party: "Customer" | "Supplier";
  obligation: string;
  sentence: (notice: string) => string;
}[] = [
  {
    party: "Customer",
    obligation: "Give notice of non-renewal",
    sentence: (notice) => `The Customer shall notify the Supplier in writing of its intention not to renew this Agreement not less than ${notice} prior to the end of the Term.`,
  },
  {
    party: "Supplier",
    obligation: "Delete customer data after termination",
    sentence: (notice) => `The Supplier shall irreversibly delete all Customer Data within its control, and shall confirm such deletion in writing, no later than ${notice} before the expiry of the Term.`,
  },
  {
    party: "Customer",
    obligation: "Request the annual audit",
    sentence: (notice) => `Should the Customer wish to exercise its audit right for the then-current year, it shall submit a written request to the Supplier at least ${notice} before the Term ends.`,
  },
  {
    party: "Supplier",
    obligation: "Notify of price changes",
    sentence: (notice) => `The Supplier may revise the Fees with effect from the next Renewal Term provided that it gives the Customer written notice of such revision at least ${notice} before the end of the Term.`,
  },
  {
    party: "Customer",
    obligation: "Return supplier equipment",
    sentence: (notice) => `All equipment supplied under Schedule 2 shall be returned to the Supplier, carriage paid, not later than ${notice} before the Term expires.`,
  },
  {
    party: "Supplier",
    obligation: "Provide the transition plan",
    sentence: (notice) => `The Supplier shall deliver a written transition plan to the Customer no less than ${notice} before the end of the Term.`,
  },
];

const NUMBER_WORDS: Readonly<Record<number, string>> = {
  10: "ten", 14: "fourteen", 20: "twenty", 30: "thirty", 45: "forty-five",
  60: "sixty", 90: "ninety", 120: "one hundred and twenty", 180: "one hundred and eighty",
};

export function generateContract(
  random: Rng,
  asOf: Date,
  runTag: string,
): GeneratedContract {
  const termEnd = addDays(asOf, random.int(40, 200));
  const selected = random.sample(OBLIGATION_TEMPLATES, random.int(4, 6));
  const noticeChoices = [10, 14, 20, 30, 45, 60, 90, 120, 180];
  const clausePrefix = random.pick(["Section", "Clause", "Article"]);
  // One clause defines a Notice Period that later clauses refer to instead of
  // restating, so at least one obligation can only be resolved by following
  // the reference.
  const sharedNotice = random.pick(noticeChoices);
  const definitionClause = `${clausePrefix} 2.1`;
  const referencingIndex = random.int(0, selected.length - 1);
  const obligations: GeneratedObligation[] = [];
  const clauseLines: string[] = [
    `${definitionClause} Definitions. "Notice Period" means a period of ${NUMBER_WORDS[sharedNotice] ?? sharedNotice} (${sharedNotice}) days.`,
    `${clausePrefix} 2.2 Term. This Agreement continues until ${isoDate(termEnd)} (the "Term").`,
  ];
  selected.forEach((template, index) => {
    const clause = `${clausePrefix} ${3 + index}.1`;
    const usesReference = index === referencingIndex;
    const noticeDays = usesReference ? sharedNotice : random.pick(noticeChoices);
    const noticeText = usesReference
      ? `the Notice Period defined in ${definitionClause}`
      : `${NUMBER_WORDS[noticeDays] ?? noticeDays} (${noticeDays}) days`;
    clauseLines.push(`${clause} ${template.sentence(noticeText)}`);
    obligations.push({
      clause,
      party: template.party,
      obligation: template.obligation,
      noticeDays,
    });
  });
  return {
    text: [
      `MASTER SERVICES AGREEMENT (${runTag})`,
      "",
      ...clauseLines,
      "",
      `${clausePrefix} 9.1 Governing law. This Agreement is governed by the laws of the Republic of Ireland.`,
    ].join("\n"),
    termEnd: isoDate(termEnd),
    obligations,
  };
}

function endOfQuarter(value: Date): Date {
  const quarter = Math.floor(value.getUTCMonth() / 3);
  return new Date(Date.UTC(value.getUTCFullYear(), quarter * 3 + 3, 0));
}
