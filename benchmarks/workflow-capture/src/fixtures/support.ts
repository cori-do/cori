/**
 * Support triage message bank.
 *
 * Every entry states a customer situation in ordinary language. The category
 * and priority are the hidden answer: neither the category name nor the
 * priority code ever appears in the surface text, and the bank deliberately
 * spreads shared vocabulary across classes so that no single literal separates
 * one class from the rest. `assertRegexResistant` enforces that property, so a
 * captured workflow cannot pass this task with a keyword matcher.
 *
 * Category and priority vary independently: an unusable service can affect one
 * team or thousands, and a product defect can be cosmetic or expose data.
 */
export interface SupportMessageTemplate {
  category: "outage" | "access" | "billing" | "bug" | "how_to";
  priority: "P0" | "P1" | "P2";
  subject: string;
  body: string;
}

export const SUPPORT_MESSAGES: readonly SupportMessageTemplate[] = [
  {
    category: "outage",
    priority: "P0",
    subject: "Nothing loads for anyone here",
    body: "Since roughly twenty minutes ago every person on our team gets a spinning wheel on the dashboard and then a red screen. We have tried three browsers and two different networks.",
  },
  {
    category: "outage",
    priority: "P0",
    subject: "Payments failing for every shopper",
    body: "Our storefront breaks at the payment step for all shoppers since this morning. We have taken zero orders in two hours and the queue is growing.",
  },
  {
    category: "outage",
    priority: "P0",
    subject: "Plus rien ne fonctionne depuis ce matin",
    body: "Aucun de nos 200 collaborateurs ne parvient a ouvrir l'application. La page reste blanche pendant une minute puis affiche un message rouge. Nous sommes totalement a l'arret.",
  },
  {
    category: "outage",
    priority: "P0",
    subject: "Komplettausfall seit heute frueh",
    body: "Seit 07:40 Uhr erreicht keiner unserer Mitarbeiter die Anwendung. Jede Anfrage laeuft in eine Zeitueberschreitung. Betroffen sind alle Standorte.",
  },
  {
    category: "outage",
    priority: "P0",
    subject: "Every report comes back empty",
    body: "Since Saturday every report anyone here runs returns nothing at all. The data is clearly still there because the counters on the home page are correct.",
  },
  {
    category: "outage",
    priority: "P1",
    subject: "Import endpoint down for our billing group",
    body: "The endpoint our billing group relies on has returned a failure on every call since last night. Nobody in that group can do any of their work. The rest of the product is behaving normally for everyone else.",
  },
  {
    category: "outage",
    priority: "P1",
    subject: "Notre equipe comptable est totalement bloquee",
    body: "Le module de rapprochement echoue systematiquement depuis hier soir. Les quatre personnes de cette equipe ne peuvent plus rien traiter. Le reste de la societe travaille sans souci.",
  },
  {
    category: "access",
    priority: "P1",
    subject: "Locked out since the reset",
    body: "I followed the reset link yesterday and now nothing I type gets me back in. I have not been able to start any of my work today and I am the only one who can approve our releases.",
  },
  {
    category: "access",
    priority: "P1",
    subject: "Ich komme seit gestern nicht mehr hinein",
    body: "Nach der Umstellung werde ich jedes Mal zurueckgeworfen. Ich kann keine einzige Aufgabe mehr erledigen und warte seit gestern frueh.",
  },
  {
    category: "access",
    priority: "P1",
    subject: "Whole finance group lost their permissions",
    body: "Overnight everyone in our finance group stopped being able to open anything they own. None of the six of them can proceed with month end.",
  },
  {
    category: "access",
    priority: "P1",
    subject: "Mon compte administrateur ne me laisse plus entrer",
    body: "Depuis la migration de vendredi, mes identifiants sont refuses. Je suis le seul administrateur et plus personne ne peut etre ajoute tant que je ne peux pas entrer.",
  },
  {
    category: "access",
    priority: "P2",
    subject: "Archive section no longer visible to me",
    body: "I can still do everything else, but the archive area I used to be able to open is not showing for me any more. There is no rush on this one.",
  },
  {
    category: "access",
    priority: "P2",
    subject: "Former colleague still appears active",
    body: "Someone who left us in March is still listed as active on our workspace. Nothing is blocked for anyone, but we would like them removed before our next review.",
  },
  {
    category: "access",
    priority: "P2",
    subject: "Berichtsbereich fehlt mir seit dem Update",
    body: "Der Auswertungsbereich taucht bei mir nicht mehr auf. Ansonsten arbeite ich voellig normal weiter, es eilt also nicht.",
  },
  {
    category: "billing",
    priority: "P1",
    subject: "Taken twice for June",
    body: "Two identical amounts left our account on the same day this month. Our bank confirms both went through and we would like one returned.",
  },
  {
    category: "billing",
    priority: "P1",
    subject: "Preleve alors que nous avions resilie",
    body: "Nous avons mis fin a notre contrat en mai et pourtant une somme a ete prelevee la semaine derniere. Merci de nous la restituer.",
  },
  {
    category: "billing",
    priority: "P1",
    subject: "Amount collected does not match our agreement",
    body: "What came out of our account this month is roughly three times what our signed agreement says it should be. Nothing about our usage changed.",
  },
  {
    category: "billing",
    priority: "P2",
    subject: "Add our tax registration to future paperwork",
    body: "Could our tax registration number be shown on the paperwork you issue us from now on? Our accountant has asked for it.",
  },
  {
    category: "billing",
    priority: "P2",
    subject: "Abrechnung lieber quartalsweise",
    body: "Koennen wir die Abrechnung kuenftig quartalsweise statt monatlich erhalten? Das wuerde unserer Buchhaltung die Arbeit erleichtern.",
  },
  {
    category: "billing",
    priority: "P2",
    subject: "Which reference should we quote",
    body: "Our purchasing department needs to know which cost centre reference to quote when they raise the order for next year. No hurry.",
  },
  {
    category: "bug",
    priority: "P0",
    subject: "Export contained another organisation's records",
    body: "One of our analysts opened a file she downloaded from the reporting screen and it listed customers that are not ours. We have told the team to stop downloading anything until we hear from you.",
  },
  {
    category: "bug",
    priority: "P0",
    subject: "Un utilisateur a vu les informations d'un autre client",
    body: "Une personne de notre equipe a ouvert son tableau de bord et y a trouve les enregistrements d'une autre societe, avec des noms et des adresses. Le reste de l'outil fonctionne normalement.",
  },
  {
    category: "bug",
    priority: "P0",
    subject: "Earlier versions gone after saving",
    body: "Saving a record replaced everything that was written before it and there is no way for us to get the earlier content back. Two weeks of notes are gone. Everything else works.",
  },
  {
    category: "bug",
    priority: "P1",
    subject: "Approval control does nothing for our operations group",
    body: "When anyone in our operations group presses the approve control, nothing at all happens. Not one of them has been able to complete a single request since Tuesday. Other groups are unaffected.",
  },
  {
    category: "bug",
    priority: "P1",
    subject: "Jeder Upload bricht ab",
    body: "Jede Datei, die unser Support-Team hochlaedt, bricht bei etwa achtzig Prozent ab. Dieses Team kann dadurch gar nicht mehr arbeiten, alle anderen Abteilungen sind nicht betroffen.",
  },
  {
    category: "bug",
    priority: "P2",
    subject: "Stray sign on the summary figures",
    body: "The totals on the overview page carry a minus in front of them even when the underlying figures are positive. Everywhere else the same numbers appear correctly, so we are just ignoring that page.",
  },
  {
    category: "bug",
    priority: "P2",
    subject: "Le tri se reinitialise",
    body: "Quand on change d'onglet, l'ordre choisi revient a celui par defaut. Il suffit de le reappliquer, ce n'est donc pas bloquant, mais c'est agacant.",
  },
  {
    category: "bug",
    priority: "P2",
    subject: "Times shown are off by two hours",
    body: "On the activity screen only, the times appear two hours behind what we expect. The same entries are right on the detail screen, so we work from that one instead.",
  },
  {
    category: "how_to",
    priority: "P2",
    subject: "Sending a summary out every Monday",
    body: "Is there a way to have the weekly summary arrive in our inboxes automatically at the start of each week rather than one of us pulling it manually?",
  },
  {
    category: "how_to",
    priority: "P2",
    subject: "Ajouter une personne a notre espace",
    body: "Quelle est la marche a suivre pour ajouter un nouveau collaborateur a notre espace de travail ? Nous accueillons quelqu'un le mois prochain.",
  },
  {
    category: "how_to",
    priority: "P2",
    subject: "Jahresuebersicht als Tabelle",
    body: "Wie erhalten wir die Jahresuebersicht in einem Format, das sich in einer Tabellenkalkulation oeffnen laesst? Wir brauchen sie fuer die Planung.",
  },
  {
    category: "how_to",
    priority: "P2",
    subject: "Changing many entries at once",
    body: "We have a few hundred entries that all need the same marker applied. Is there something better than opening each one, or should we just work through them?",
  },
  {
    category: "how_to",
    priority: "P2",
    subject: "Finding last quarter's history",
    body: "Where would we look to see who changed what during the previous quarter? Our reviewer has asked us to walk them through it next week.",
  },
];

export const SUPPORT_SENDERS: readonly string[] = [
  "dana.holt@northwind.example",
  "p.leroux@meridian.example",
  "s.bergmann@altmark.example",
  "j.okafor@brightpath.example",
  "m.rossi@lucania.example",
  "k.svensson@nordvik.example",
  "a.dubois@calvet.example",
  "t.yamada@kaisei.example",
  "r.oconnell@ferngrove.example",
  "l.marchetti@vespera.example",
  "c.nowak@wisla.example",
  "e.fontaine@auberive.example",
  "h.mueller@steinbach.example",
  "n.alvarez@puentes.example",
  "b.whitfield@larkmoor.example",
  "g.petrov@danubia.example",
  "i.haugen@fjordly.example",
  "v.kaur@amberline.example",
];
