/// Everything that needs explaining, in one place.
///
/// The panels used to carry this prose themselves, which made every step three paragraphs tall and
/// still left the important parts unsaid. Keeping it here means a stage can be a control and a
/// result, and anyone who wants the reasoning can open it beside them.
export type HelpSection = {
  id: string;
  title: string;
  body: React.ReactNode;
};

export const sections: HelpSection[] = [
  {
    id: "what",
    title: "What Orbiter does",
    body: (
      <>
        <p>
          A company build is signed by the company's Apple team. iOS installs it
          only on iPhones registered to that team, and a team is allowed 100
          devices per year. Once that allowance is full, a tester's iPhone
          cannot take the build at all — not because anything is wrong with it,
          but because there is no room left to register them.
        </p>
        <p>
          Orbiter takes that build apart and rebuilds it under the tester's own
          free Apple ID: new bundle identifiers, their certificate, their
          provisioning profile, every nested bundle re-signed. The result
          installs on their phone without touching the company's allowance.
        </p>
        <p>
          Your original IPA is opened read-only and never modified. The
          re-signed build is a separate file.
        </p>
      </>
    ),
  },
  {
    id: "losses",
    title: "What the re-signed build cannot do",
    body: (
      <>
        <p>
          Apple grants a free personal team no capabilities at all. It is not a
          setting and not something Orbiter chooses — each bundle is signed with
          exactly the entitlements inside Apple's own profile for it, and that
          profile authorises none of these:
        </p>
        <ul>
          <li>
            <strong>Push notifications</strong> — the app never registers with
            APNs, so notifications simply never arrive.
          </li>
          <li>
            <strong>Universal links</strong> — links that would open the app
            open in Safari instead, and web credential autofill stops.
          </li>
          <li>
            <strong>Apple Pay</strong> — merchant identifiers are not authorised
            for this team.
          </li>
          <li>
            <strong>App groups</strong> — the app and its extensions can no
            longer share storage.
          </li>
          <li>
            <strong>Keychain items</strong> saved under the company team are
            unreadable, because the access group moved with the identifier.
          </li>
        </ul>
        <p>
          These fail silently. The app looks fine and notifications just never
          come. Tell testers, or they will report it as a bug.
        </p>
        <p>
          Separately, the build installs under a <em>rewritten</em> bundle
          identifier, because Apple will not let another team register the
          company's one. Anything that recognises the app by that identifier
          will not recognise this build: social sign-in providers, and commonly
          a company's own backend. Registering the new identifier with those
          services fixes it. Each tester's team produces a different identifier,
          so each is registered separately.
        </p>
      </>
    ),
  },
  {
    id: "seven-days",
    title: "The seven-day limit",
    body: (
      <>
        <p>
          A free personal team's provisioning profile expires seven days after
          Apple issues it. When it does, the app stops launching on the tester's
          phone — it does not warn them first, and reinstalling the same file
          does not help because the expiry is inside the profile.
        </p>
        <p>
          The fix is to re-sign and reinstall, which issues a fresh profile.
          Orbiter does not do this on a schedule; nothing here contacts Apple
          without someone clicking.
        </p>
        <p>
          The identifier stays the same each week, because it is derived from
          the team. The tester keeps one app that gets replaced, rather than
          collecting a new icon every week.
        </p>
        <p>
          Orbiter remembers when a build it installed runs out and says so on
          the IPAs page, counting down in whole days and then announcing it once
          the app has stopped launching. It keeps only what that line needs: the
          app's name, its identifier, the expiry, and a tag standing in for the
          team. Not the IPA's location on this Mac, not the phone, not the Apple
          ID. <em>Forget</em> on that line deletes all of it; nothing on any
          phone changes.
        </p>
      </>
    ),
  },
  {
    id: "untrusted",
    title: '"Untrusted Developer" on the iPhone',
    body: (
      <>
        <p>
          iOS will not run a build signed by a personal team until the
          certificate is trusted on the device itself. The first launch shows
          <em> Untrusted Developer</em>.
        </p>
        <p>
          On the iPhone:{" "}
          <strong>Settings → General → VPN &amp; Device Management</strong>,
          choose the developer entry for the Apple account that signed it, then{" "}
          <strong>Trust</strong>. The phone needs internet access to verify it.
        </p>
        <p>
          This is asked once per certificate, not once per app or per week. The
          following week's build uses the same certificate and launches without
          asking again.
        </p>
      </>
    ),
  },
  {
    id: "account",
    title: "The Apple account, and what is stored",
    body: (
      <>
        <p>
          Orbiter authenticates directly with Apple over HTTPS, using macOS's
          own authentication support. There is no proxy, no remote server, and
          no fallback to one.
        </p>
        <p>
          Passwords and verification codes are never saved. The account session
          lives in memory and expires after 30 minutes or when Orbiter closes.
          The signing key is generated on this Mac, kept in its Keychain, and
          never leaves — only a certificate request goes to Apple.
        </p>
        <p>
          Every action that writes to the Apple account — registering a device,
          requesting a certificate, registering identifiers — is behind its own
          acknowledgement, and none of them happens on its own. Orbiter never
          withdraws a certificate unless you ask it to, because that stops every
          app already signed with it from launching.
        </p>
        <p>
          Local authentication uses private macOS frameworks that may change
          between releases; if it stops working, sign-in stops rather than
          falling back to something less private. Windows is not implemented.
        </p>
      </>
    ),
  },
  {
    id: "diagnose",
    title: "When something in the app does not work",
    body: (
      <>
        <p>
          First check whether it depends on a capability the team cannot carry —
          push, universal links, Apple Pay, app groups — or on a service that
          knows the old bundle identifier. Those are expected, not faults.
        </p>
        <p>
          For anything else, use <strong>Device log</strong> after signing. It
          streams the iPhone's log while you reproduce the problem and keeps
          only the lines about this app; everything else the device logs is
          counted and discarded. iOS names the exact entitlement or request it
          refused.
        </p>
        <p>
          One blind spot: a screen built as a web view. Web content failures — a
          JavaScript error, a blocked request, a rejected API call — never reach
          the device log; it records only that the web process ran. Use Safari's
          Web Inspector for those: enable{" "}
          <strong>
            Safari → Settings → Advanced → Show features for web developers
          </strong>
          , then <strong>Develop → your iPhone → the app</strong>. A re-signed
          build can be inspected this way; a company distribution build cannot.
        </p>
      </>
    ),
  },
];
