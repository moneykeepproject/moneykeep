# Phone capture

Phone capture lets you photograph receipts and enter expenses on your phone. They are
kept on the phone first (in its **Outbox**) and sent to MoneyKeep on your computer,
which files them into your book. Your book never leaves your computer.

There are two ways for the phone to reach your computer:

- **Wi-Fi capture** (the normal setup, described first in this guide): the phone talks
  to your computer directly over your home or office Wi-Fi. No Cloudflare account, no
  domain, nothing on the internet. You can enter expenses anywhere; they are sent the
  next time the phone is on the same Wi-Fi as your computer and you open the app.
- **Phone capture over Cloudflare** (in Settings > Phone capture after the Wi-Fi
  part, set up under Settings > Advanced; described at the end): the phone reaches
  your computer through Cloudflare from anywhere. It needs your own domain on
  Cloudflare.

This guide is written for someone who has never set up a network device. Take it one
step at a time; each step says what to check before going on.

```
Phone (on your Wi-Fi) ──HTTPS──> https://192.168.1.20:14443   phone app and its API
        │                         (a certificate your phone trusts once)
        └──HTTP──> http://192.168.1.20:14080   only the certificate page and the
                                               certificate itself
                     moneykeep-server on this computer
                               ▲
          MoneyKeep desktop ───┘ on this computer only (127.0.0.1)
```

## Contents

1. What you need
2. Give the computer a fixed address
3. Run Wi-Fi setup in MoneyKeep
4. Install the certificate on the phone
5. Pair the phone
6. Everyday use
7. If the computer's address changes
8. Removing
9. Troubleshooting
10. Security notes
11. When to use Cloudflare instead

Then: Advanced: phone capture over Cloudflare.

## 1. What you need

- **The computer with MoneyKeep**, turned on, with MoneyKeep running (or the capture
  service installed by the setup, which starts with the computer). It must be on the
  **same network** as the phone: the same Wi-Fi, or a cable to the same router.
- **A phone** with **Chrome** (Android) or **Safari** (iPhone or iPad). Other browsers
  may work, but the steps below use these two.
- **A few minutes.** You do steps 2 and 3 once on the computer, and steps 4 and 5 once
  on each phone.
- **For an iPhone or iPad: a passcode** on the phone. iOS asks for it to install the
  certificate.
- **MoneyKeep installed with its setup** (`moneykeep-<version>-windows-x64-setup.exe` on
  Windows, or MoneyKeep Launcher). The setup puts the capture server program (`moneykeep-server`) next to
  MoneyKeep, built together with it, so you never build anything. If MoneyKeep says
  the program is missing, or that this copy "was built without its capture server",
  install MoneyKeep with its setup (developers: build with `cargo xtask release`;
  see "Building MoneyKeep with its capture server" in the Advanced part below).
- In MoneyKeep, **Features > Expenses** must be turned on.

## 2. Give the computer a fixed address

### Why the address must not change

Every device on your network has an address, such as `192.168.1.20`. Your phone
keeps the capture app, its pairing with your computer and any expenses not yet sent
**for one address**. If the computer's address changes, the phone sees a different
app: it would need the certificate again and a new pairing, and expenses still in
the old app stay there. So before you start, make sure the computer keeps its
address.

Most home routers hand out addresses automatically (this is called **DHCP**) and may
give the computer a different one after a restart. The fix is a **DHCP
reservation**: you tell the router "always give this computer this address". The
router recognises the computer by its network adapter's **MAC address** (six pairs
of letters and digits, such as `a4:b1:c2:d3:e4:f5`). MoneyKeep shows the address and
the MAC address of each adapter in Wi-Fi setup's first step and in **Details**.

Pick the way that matches your network:

### Home router

1. Find your router's admin page. The address and the password are usually printed
   on a sticker on the router (for example `http://192.168.1.1` or
   `http://192.168.0.1`). Some routers use an app from the internet provider
   instead.
2. Sign in and look for a page called **DHCP reservation**, **Address reservation**,
   **Static lease**, **Fixed IP**, **IP & MAC binding** or **Reserved IP**. It is often
   under LAN, Network, DHCP server or Advanced.
3. Add a reservation: choose the computer from the list of connected devices (or
   type its MAC address), and keep the address it has now — the one MoneyKeep shows.
4. Save. Some routers apply it at once; others after the computer reconnects.
5. Restart the computer, open MoneyKeep, Settings > Phone capture > Set up Wi-Fi
   capture, and check that the address is still the same.

If you cannot find the page, search the web for your router's model name and "DHCP
reservation".

### OPNsense or pfSense firewalls

1. Open **Services > DHCPv4 >** your LAN interface (in newer versions with Kea or
   dnsmasq, the matching "Reservations" or "Hosts" page).
2. Add a **static mapping**: the computer's MAC address, the IP address it should
   always get (inside your LAN, ideally outside the dynamic pool), and a description
   such as "MoneyKeep computer". Save and apply.
3. Optional, and only if you want a name instead of the number (see "A local name"
   below; a fixed IP address needs no name): in **Services > Unbound DNS >
   Overrides** (Host overrides) add host `moneykeep`, domain `home.arpa`, with the
   same IP address. `home.arpa` is the name reserved for home networks (RFC 8375).
   Then enter `moneykeep.home.arpa` as the local name in Wi-Fi setup.
4. Make sure the firewall rules of the Wi-Fi network allow devices to reach the
   computer on TCP ports 14443 and 14080 (a guest network with isolation blocks this).

### Corporate or office network

If on a corporate network, request a fixed IP address via DHCP reservation from your
network team. Do not change network settings yourself. Ask your network or IT team
for a **fixed IP address via a DHCP reservation** for this computer: give them its
MAC address, shown in MoneyKeep's Wi-Fi setup **Details**, and say which adapter
(Wi-Fi or Ethernet) you use. Ask them too whether phones on the Wi-Fi may reach this
computer on **TCP ports 14443 and 14080** — many office and guest Wi-Fi networks keep
devices from reaching each other ("client isolation"). If they cannot allow it, use
Cloudflare instead (Settings > Advanced, section 11).

### A static address set on the computer itself

You can also type a fixed address into the computer's own network settings. Do this
only if you know your network's free address range: an address the router also hands
to another device makes both stop working. A reservation in the router (above) is
better.

### Laptops: Wi-Fi and Ethernet

A reservation belongs to one network adapter. A laptop's Wi-Fi and its Ethernet
port (or a USB or dock adapter) are different adapters with different MAC addresses
and different addresses. Reserve the one you normally use, and choose the same one
in Wi-Fi setup. If you sometimes use the other, the phone cannot reach the computer
then; it simply waits and sends later.

### A local name (optional — leave it blank)

Leave the name blank unless your network has its own DNS (an OPNsense or router host
override, or your company's DNS). With a fixed IP address no name is needed.

A name such as `moneykeep.home.arpa` works only when something on your network
answers for it: the phone asks your router (or your company's DNS server) where the
name leads, and most home routers do not know names you make up. If nothing answers,
the phone cannot find the computer, while the IP address always works. When you type
a name, Wi-Fi setup checks that it leads to the chosen address on this computer; if
it does not, it says "This name does not lead to <address> on this computer. Leave
it blank to use the address." and goes on only if you press **Use it anyway**.

To drop a name already in use: Settings > Phone capture > **Change address…**, keep
the address, clear the name and press Change address. MoneyKeep makes a new
certificate for the address alone. A phone keeps the app, its pairing and its Outbox
per address, so to the phone this is a new app. Before you change it, send everything
in the phone's Outbox. Afterwards install the new certificate, open the new address on
the phone, install the app again and pair it. The app at the old address shows
nothing new; its home-screen icon can be removed.

## 3. Run Wi-Fi setup in MoneyKeep

Open **Settings > Phone capture** and press **Set up Wi-Fi capture…**. The dialog
has six steps. You can close it at any time; it opens again where you stopped.
**Details** lists what was made; **Help** shows this guide.

1. **Address.** MoneyKeep lists this computer's addresses with the adapter names
   (and marks a network Windows treats as Public). Choose the one on the network
   your phone uses — usually the Wi-Fi or Ethernet adapter with an address starting
   with `192.168.`, `10.` or `172.`. Leave **Local name (optional — leave blank)**
   empty: blank is best, and the IP address is used for the certificate and pairing.
   Type a name only if your router or DNS server answers for it (see "A local name"
   above). The ports are 14443 (HTTPS) and 14080 (certificate page) unless another
   program already uses them.
   - "This address is not one of this computer's addresses now": the computer is
     not connected to that network, or the address changed. Connect it, or choose
     the address shown.
   - "This name does not lead to <address> on this computer": clear the name to use
     the address (best), or press Use it anyway if you know the phones' network
     answers for it.
2. **Capture PIN.** Choose 6 to 12 digits and type them twice. Phones enter this PIN
   once, when they pair. It is not your book password: the phone never needs the
   key to your book. MoneyKeep's capture server keeps only a one-way hash of it
   (Argon2id), never the PIN itself. You can change it later (Change PIN); phones
   already paired keep working.
3. **Firewall.** On Windows, the next step adds an inbound firewall rule named
   **MoneyKeep capture (Wi-Fi)** for the two ports, for **Private and Domain
   networks only**. If Windows treats your network as **Public**, the rule does not
   apply and phones cannot connect: open Windows **Settings > Network & internet >
   Wi-Fi** (or Ethernet) **>** your network, and set **Network profile type** to
   **Private network**. On macOS, if the firewall is on, macOS asks once whether
   `moneykeep-server` may accept incoming connections: choose **Allow** (see "On a
   Mac" below). On
   Linux, if a firewall such as ufw or firewalld is on, allow TCP ports 14443 and
   14080 from your home network, for example
   `sudo ufw allow from 192.168.0.0/16 to any port 14443,14080 proto tcp`.
4. **Start.** Press **Start**. Windows asks once for permission (to install or
   update the capture service and add the firewall rule). MoneyKeep then:
   - installs (or updates) the capture server as a service that starts with the
     computer, with the Wi-Fi listener on the chosen address;
   - makes a small **certificate authority** for this computer ("MoneyKeep Capture
     CA (<computer name>)") and a certificate for the address — the private keys stay
     in the computer's protected storage and never leave it;
   - adds the firewall rule and sets the Capture PIN;
   - checks that the server answers on the address.
   If the check fails: "port already used" names the port and the next free one —
   change it in step 1; "address not on this computer" — see step 1; otherwise see
   Troubleshooting.
5. **Certificate on the phone.** MoneyKeep shows **QR code 1** and the
   certificate's **fingerprint** (8 groups of 4 characters). Follow section 4 on the
   phone, then press Next.
6. **Pair.** MoneyKeep shows **QR code 2** and a pairing code that lasts 10 minutes.
   Follow section 5. When the phone has paired, the dialog shows **Paired: <name>**.

## 4. Install the certificate on the phone

**Why this is needed, and why it is safe.** A phone only installs a web app, keeps it
for offline use and keeps its storage when the address is secure (HTTPS). Secure
addresses need a certificate the phone trusts. Your computer makes its own small
certificate authority, and each phone trusts it once. It is **limited to your
computer's address** (and local name): the phone will not accept it for any other
website, so it cannot be used to read your other traffic. The certificate page is
plain HTTP, so **compare the fingerprint** the phone shows with the one in MoneyKeep
before you trust it.

### Android

Wording varies a little between phone makers and Android versions.

1. Scan **QR code 1** with the camera and open the link (it starts with `http://` and
   ends with `:14080/`). The certificate page shows the fingerprint; compare it with
   MoneyKeep's.
2. Tap **Download certificate (Android)**. If Chrome asks, keep the file.
3. Open **Settings > Security** (or **Security & privacy**) **> More security
   settings > Encryption & credentials**. Searching Settings for "certificate" also
   finds it.
4. Tap **Install a certificate > CA certificate**, read the warning, then **Install
   anyway**.
5. Choose the downloaded file **moneykeep-ca.crt** (in Downloads). The phone may ask
   for its screen lock (PIN, pattern or fingerprint).
6. Check under **Trusted credentials > User** that **MoneyKeep Capture CA** is
   listed. Tapping it shows its details, including the SHA-256 fingerprint: its
   first 32 characters are the ones MoneyKeep shows.

### iPhone or iPad

1. Scan **QR code 1** with the **Camera** app and tap the link; it opens in **Safari**
   (use Safari: other browsers cannot install profiles). Compare the fingerprint
   with MoneyKeep's.
2. Tap **Download certificate (iPhone or iPad)**, then **Allow**. iOS says "Profile
   Downloaded".
3. Open **Settings**. Tap **Profile Downloaded** near the top (or **General > VPN &
   Device Management** > MoneyKeep Capture CA), then **Install**, enter the phone's
   passcode, and **Install** again. The profile is shown as unsigned; that is
   expected for a certificate your own computer made.
4. Open **Settings > General > About > Certificate Trust Settings** (at the very
   bottom) and turn on full trust for **MoneyKeep Capture CA**. Without this step
   Safari still says the connection is not private.
5. Check the fingerprint: General > VPN & Device Management > the profile > More
   Details shows the SHA-256 fingerprint; its first 32 characters match MoneyKeep's.

## 5. Pair the phone

1. In MoneyKeep, go to the dialog's **Pair** step (or Settings > Phone capture >
   Paired phones > **Pair a phone…**). It shows **QR code 2**, and under "Can't
   scan?" the pairing code to type instead.
2. Scan it with the phone. The capture app opens at `https://<address>:14443/`.
3. Type the **Capture PIN** (the keypad shows digits; tap the eye to see what you
   typed) and a **name for this phone**, such as "Anna's Pixel". Tap **Pair**.
   - "That PIN is not right. Check it in MoneyKeep on your computer.": try again.
     After 5 wrong PINs the code stops working; press **New code** in MoneyKeep.
4. MoneyKeep shows **Paired: <name>**. The phone shows the Expense screen.
5. **Install the app on the home screen**, so it opens like an app, works without a
   connection and keeps its pairing:
   - **Android (Chrome):** tap **Install app** in the capture app, or Chrome's menu ⋮ >
     **Install app** (or Add to Home screen).
   - **iPhone or iPad (Safari):** tap **Share > Add to Home Screen**, open the new
     icon, and **pair again inside the installed app** with a new code (Pair a
     phone…). On iPhone the installed app has its own storage, separate from
     Safari's.

## 6. Everyday use

- **Entering expenses** works anywhere, also without any connection: everything is
  saved on the phone first, in the **Outbox**.
- **Sending** happens when the phone can reach your computer: the app tries when it
  opens, when the phone comes back online, when you switch back to the app, and every
  minute while it is open. Web browsers do not let a closed web app send in the
  background reliably, so: **once you are home, open the app** and it sends by
  itself.
- **The status light** in the phone's top bar: green — connected; yellow "Your
  computer isn't reachable" — the computer cannot be reached (not on the same Wi-Fi,
  or the computer is off, asleep or offline); red — the phone is offline. While items
  wait, the Outbox says "Will send when you are on the same Wi-Fi as your computer"
  (over Cloudflare: "Will send when your computer is on and connected").
- **On the computer**, MoneyKeep collects what the phones sent every 15 minutes while
  a book is open (and on Collect now), and the capture service keeps everything
  until then, even when MoneyKeep is closed. The computer must be on and awake for
  phones to send.
- The certificate of the server renews itself automatically before it ends; nothing
  changes on the phone. The certificate authority lasts 10 years.

### When your computer is off

- **The app still opens** on the phone — over the Wi-Fi or over Cloudflare — if it
  has been opened once before: the phone keeps a copy of the app. Instead of an error
  page you see the app with the yellow "Your computer isn't reachable".
- **New entries wait in the Outbox.** You can keep adding expenses and trips; nothing
  is lost and the phone stays paired. The lists from your book (accounts,
  categories, tags, vehicles, saved trips) are the ones from the last time the phone
  reached the computer; a small note says when, for example "Lists from Nov 8, 1:05
  PM".
- **Open the app again after the computer is back on.** Web browsers do not send in
  the background reliably, so the Outbox sends when the app is open.
- **To have it always reachable**, keep the computer on and set Windows so it doesn't
  sleep while plugged in: in Settings > System > Power & battery (Power & sleep on
  Windows 10), under the screen and sleep settings, set sleep when plugged in to
  **Never**. The screen may still turn off.

### On a Mac

- **Phone capture works while you're logged in.** On a Mac the capture server runs as
  your own background agent, not for the whole computer, so it needs no password.
  On a Mac, phone capture works while you're logged in. Expenses sent while you're
  logged out wait on your phone, and are sent when you log in again and open the app
  on the phone. Switching to another user (fast user switching) keeps it running.
- **The server runs from MoneyKeep.app.** If you move MoneyKeep to another folder,
  Settings > Phone capture says the capture server isn't installed: press **Install
  the server again** (paired phones and waiting expenses are kept).
- **The firewall.** If macOS's firewall is on (System Settings > Network >
  Firewall), macOS asks once whether `moneykeep-server` may accept incoming
  connections: choose **Allow**. A MoneyKeep without a Developer ID signature looks
  like a new program to macOS after each update, so macOS may ask again; choose Allow
  again. If you chose Deny, or phones can't reach this Mac while the firewall is on,
  the Status card says "macOS's firewall may be blocking phone capture" with **Open
  Firewall settings**: there, under Options, set `moneykeep-server` to allow
  incoming connections.
- **To have it always reachable**, stay logged in and keep the Mac from sleeping:
  System Settings > Battery (or Energy) > Options, turn on "Prevent automatic sleeping
  on power adapter when the display is off".

## 7. If the computer's address changes

If the router gave the computer a new address (no reservation, a new router, a
move):

1. **Before you change it, send everything in the phone's Outbox** (each phone, on the
   old address), if you can.
2. In MoneyKeep: Settings > Phone capture > **Change address…**. Read the warning
   (every phone will need the new certificate installed again), choose the new
   address and press Change address. MoneyKeep makes a new certificate (and a new
   certificate authority, because it is limited to the one address it was made
   for), and lists the phones that have not been in contact for over a day.
   Afterwards the Status card lists the phones paired over Wi-Fi that haven't
   connected since the change, with **Certificate QR code…**, until each one has.
3. **Afterwards install the new certificate, open the new address on the phone,
   install the app again and pair it** (sections 4 and 5). The app at the old address
   is a different app to the phone and shows nothing new; anything still in its
   Outbox stays there.
4. Remove the old app from the phone: Android — long-press its icon > Uninstall (or
   App info > Uninstall); iPhone — long-press > Remove App > Delete. Remove the old
   certificate too (section 8).

Then set up a DHCP reservation (section 2) so it does not happen again. Wi-Fi setup
and Change address remind you whenever you choose an address.

## 8. Removing

- **A phone:** in MoneyKeep, Settings > Phone capture > Paired phones > **Revoke**.
  The phone can no longer send. Anything still on it stays there.
- **The app on the phone:** Android — long-press the icon > Uninstall; iPhone —
  long-press > Remove App > Delete App.
- **The certificate on the phone:** Android — Settings > Security > Encryption &
  credentials > **Trusted credentials > User** > MoneyKeep Capture CA > Remove;
  iPhone — Settings > General > **VPN & Device Management** > the MoneyKeep profile >
  **Remove Profile**.
- **Wi-Fi capture altogether:** Settings > Phone capture > **Remove Wi-Fi
  capture…**. MoneyKeep stops the Wi-Fi listener, removes the firewall rule, the
  certificates and their keys, and disconnects the phones paired over Wi-Fi (after a
  warning). Phone capture over Cloudflare, if set up, keeps working.

### Removing phone capture from a Mac

Before you drag MoneyKeep to the Trash, open Settings > Phone capture > **Remove
phone capture from this Mac…**. It stops the capture server and removes its
background agents (and `cloudflared`'s, if MoneyKeep set it up for Cloudflare), with
no password. It offers to delete the capture server's data too (paired phones, the
Wi-Fi certificate and expenses not yet collected); the default keeps it, for a later
reinstall. Its keychain item "MoneyKeep capture server" stays in your login
keychain; delete it in Keychain Access if you deleted the data. If you deleted the
app first, the agent is left pointing at a program that is gone and macOS stops
trying to start it; to remove it, delete
`~/Library/LaunchAgents/com.moneykeep.capture-server.plist` (and
`com.moneykeep.cloudflared.plist`) and log out and in.

### Updating MoneyKeep

Run the new setup: it updates MoneyKeep in place ("MoneyKeep 0.9.2 will be updated to
0.9.3. Your books and settings are kept."). When the capture service runs from
MoneyKeep's folder, the setup stops it, replaces it with the new one and starts it
again, so nothing more is needed. If MoneyKeep is open, the setup asks you to close it.

If the capture server still runs an older version after an update (for example it
was installed from another folder), MoneyKeep says so: Home > Attention shows **A
newer capture server is ready.**, and Settings > Phone capture shows **The capture
server needs an update** with **Update now** and **Later**. Update now replaces the
server with the one that came with MoneyKeep and restarts it; it takes a few seconds,
phones keep their pairing, and anything in a phone's Outbox is sent afterwards.
Windows asks for permission first, because the capture server runs in the background
for all users of the computer. If you say No, nothing changes ("Not updated — Windows
permission was not given …"); press Update now when ready. Later hides the Home
reminder until MoneyKeep is opened again. MoneyKeep never updates the server unless
you press Update now.

The server only counts as older when the parts it is built from changed (the server,
the phone app and the code they share): an update that changes only the desktop
program asks for nothing. When the running server is too old for this MoneyKeep
altogether, the Status card says **Update the capture server**.

Updating keeps the previous server program beside the new one until the new one
answers its health check, and puts it back if it doesn't. The capture service restarts
itself after a failure (after 10 seconds, 30 seconds, then a minute).

### Going back to an earlier version

Downgrades aren't supported. The capture server's data folder carries a format
number, and an older MoneyKeep meeting data made by a newer one refuses to start
rather than change it: the Status card says **Phone capture isn't working: the
capture server refused to start** with "This phone capture data was made by a newer
MoneyKeep. Install that version or a newer one." To go back to an earlier version
anyway, uninstall MoneyKeep answering **Yes** to removing the capture server's data,
install the earlier version, set up phone capture again and pair the phones again.

### Where the capture server keeps its data (Windows)

The capture service runs under its own Windows account, `NT SERVICE\MoneyKeepCapture`,
not as the all-powerful LocalSystem. Its data (the inbox, paired phones, the Wi-Fi
certificates and their keys, `server.toml`) is in
`C:\ProgramData\MoneyKeep\capture-server`, a folder only that account, SYSTEM and
administrators can open. MoneyKeep itself never opens it: it talks to the server over
the server's own connection, and every change to the folder goes through the one
Windows permission prompt.

Earlier versions kept the data in your user folder
(`%LOCALAPPDATA%\MoneyKeep\data\capture-server`). The first time the setup or
MoneyKeep's service helper runs after an update, it stops the service, copies the
server's files into the new folder (file contents only; a link there is skipped and
listed), points the service at the new folder and starts it again. Phones stay paired
and the Wi-Fi certificate does not change. Before it copies anything it puts this
version's server program in the service's folder, so an older program never meets the
moved data; then it checks that the server answers on the new folder. If anything
fails, the service goes back to the old folder with its old program and the old
server is started again; Setup shows what happened ("phone capture needs attention in
MoneyKeep"). Once it worked, the old folder is left in place, marked as moved (its
`server.toml` is renamed `server.toml.moved`), and never used again: Settings > Phone
capture > Status offers **Remove the old copy** for 30 days, and after that the
**About MoneyKeep** dialog (the **i** button) offers it under **Clean up** while the
folder is there.

The inbox key in that folder is sealed by Windows for this Windows installation: a
copy of the data folder taken to another PC (or to this PC after Windows was
reinstalled) can't open it, so phones must be paired again there.

If the capture service is missing (for example MoneyKeep was uninstalled keeping the
data, then installed again) but its data is still on the computer, the Status card
says "Phone capture isn't working: the capture server isn't installed" and offers
**Install the server again**. After one Windows permission prompt it copies the data
from your user folder into the protected folder when it isn't there yet (the inbox key
is sealed by Windows there), installs and starts the service, and puts Wi-Fi capture's
firewall rule back with the same ports. Paired phones, the Wi-Fi certificate and
expenses not yet collected are kept, and **Remove the old copy** is offered as above.
If it fails, the service is removed again and your old folder is not changed. The
installer itself never installs a service you had removed.

If `C:\ProgramData\MoneyKeep` or the data folder already exists but is a link or
junction, or is owned by someone other than SYSTEM or Administrators, MoneyKeep
refuses to use it: "The capture server's folder isn't safe to use: <folder>". For
`C:\ProgramData\MoneyKeep` itself it adds "Remove the folder
C:\ProgramData\MoneyKeep (it was not made by MoneyKeep's setup) and try again", and
Setup shows it. Delete that folder (as administrator) and run the setup again.

### Uninstalling MoneyKeep

Settings > Apps > Installed apps > MoneyKeep > Uninstall. The uninstaller removes the
programs, the shortcuts and the capture service with Wi-Fi capture's firewall rule,
then asks "Also remove the capture server's data (certificates and paired phones)?"
(No keeps it, for a later reinstall). Yes removes the data folder under
`C:\ProgramData\MoneyKeep` and the old copy in your user folder, if it is still there —
each only when it is a real folder (not a link or junction) in its usual place;
otherwise the details say it was kept. With Yes it also removes the tunnel token file
MoneyKeep kept for `cloudflared` and, when MoneyKeep installed `cloudflared`'s
service, stops and removes that service. The old copy checked is the one of the user
who started the uninstall, even when an administrator's account gives the
permission. Your books are never touched.

## 9. Troubleshooting

**"Wi-Fi capture can't start: its certificate files are damaged"**

The Wi-Fi certificate's files in the capture server's data folder can't be read (a
full disk or a power cut while they were written by an older version). The server
keeps working without Wi-Fi and never replaces the certificate by itself. Press
**Make a new certificate** on the Status card: every phone then needs the certificate
installed again (section 4); pairings and waiting expenses are kept.

**"Windows blocked a PowerShell step (it may be restricted by your organisation)"**

MoneyKeep's setup uses PowerShell only for the folder permissions and the firewall
rule. On a computer managed by an organisation, ask its IT team to allow it, or to
make the firewall rule for you (section 3).

**"This site can't be reached" / "Safari can't connect to the server"**

- Is the phone on the **same Wi-Fi** as the computer? Not on mobile data (turn mobile
  data off for a moment to test), not on a guest Wi-Fi.
- Is the computer **on and awake**, and is the capture service running? MoneyKeep's
  status light should be green or yellow, not red ("the server isn't running" — press
  Start the server; "the capture server isn't installed" — press Install the server
  again).
- Is the address still the same? MoneyKeep's light says "Wi-Fi capture: this
  computer no longer has <address>" when the address went away.
- Windows: is the firewall rule **MoneyKeep capture (Wi-Fi)** present (Windows
  Security > Firewall & network protection > Advanced settings > Inbound Rules), and
  is the network **Private**, not Public (section 3, step 3)?
- **VPN apps**, **Private DNS** (Android: Settings > Network > Private DNS — try
  "Off" or "Automatic") or **ad-blocker apps** on the phone can block local
  addresses. Turn them off to test.
- **Guest Wi-Fi or office Wi-Fi isolation** ("client isolation", "AP isolation")
  keeps devices from reaching each other. Use the main Wi-Fi, ask your network team,
  or use Cloudflare.
- Some mesh or range-extender systems put devices on separate networks; connect the
  phone to the main router's Wi-Fi.

**"Can't reach your computer on this Wi-Fi"** (the phone's loading screen, or the
yellow note in the app when it opened from what it keeps)

- The phone got no answer at all within 8 seconds. Check the points above: the same
  Wi-Fi, the computer on, MoneyKeep running there. **Try again** loads the app
  again; **Use Cloudflare** (shown when phone capture over Cloudflare is set up too)
  opens the app through your Cloudflare address instead.
- On the computer, Settings > Phone capture > Status checks the firewall and says
  what it finds:
  - "Phones can't reach this computer: the firewall rule is missing." — press
    **Fix** (Windows asks for permission). MoneyKeep also asks once when it starts,
    and Setup puts the rule back by itself when MoneyKeep is installed again
    (uninstalling removes it).
  - "Windows is blocking the capture server" — Windows made its own rule that
    blocks `moneykeep-server.exe` (this happens when its "allow access" pop-up is
    closed without allowing). **Fix** removes that Block rule; Windows' Allow rules
    are left alone.
  - "Windows treats this network as public, so phones can't connect over Wi-Fi" —
    **Open network settings** and set this network's profile type to **Private
    network**. MoneyKeep doesn't change it itself.

**"Your connection is not private" / "This Connection Is Not Private"**

- The certificate is not installed, or not trusted: repeat section 4. On iPhone the
  **Certificate Trust Settings** switch is the step most often missed.
- Wrong address: the phone must open exactly the address of QR code 2 (with
  `https://` and `:14443`). An old bookmark may point to an old address.
- The phone's **date and time** are far off: set them to automatic (Settings > Date &
  time). The certificate is valid from one day before it was made, so small
  differences are fine.

**"That PIN is not right"**: check the Capture PIN in MoneyKeep (Change PIN if you
forgot it). After 5 wrong PINs the pairing code ends; make a new code.

**The app will not install / Install app is missing**

- Android: use Chrome; the certificate must be trusted first (the address must show
  a lock, not a warning). The button can take a few seconds to appear on the first
  visit; reload once if needed, or use Chrome's menu ⋮ > Install app.
- iPhone: there is no Install button; use Share > Add to Home Screen in Safari, then
  pair inside the installed app.
- Already installed: the button is not shown inside the installed app.

**Expenses stay in the Outbox**

- Open the app while on the computer's Wi-Fi and wait a minute; it sends by itself.
- If an item says it needs attention, open it: the server refused something (for
  example a photo type) and says why.
- "This phone was disconnected": the phone was revoked or Wi-Fi capture was moved or
  removed. Pair again; the Outbox is kept and sent after pairing.

## 10. Security notes

- **The device key**: when a phone pairs, the server gives it a long random token.
  Every request from the phone carries it; the server keeps only a hash of it.
  Revoking a phone ends its token.
- **The Capture PIN** protects pairing: anyone on your network still needs a pairing
  code shown on your computer (valid 10 minutes, ended after 5 wrong tries) **and**
  the PIN. The server keeps only an Argon2id hash of the PIN. Once a PIN is set,
  pairing over Cloudflare asks for it too.
- **The certificate authority is limited** (X.509 name constraints) to your
  computer's address and local name, and signs nothing else. Its private key stays on
  the computer, in the operating system's credential store (or, for the Windows
  service, in an encrypted file in the capture server's data folder); it is never
  sent to a phone or written to a log.
- **The service's own account (Windows)**: the capture server runs as
  `NT SERVICE\MoneyKeepCapture` with its data in a folder only it, SYSTEM and
  administrators can open (see "Where the capture server keeps its data"). The key
  that encrypts the inbox and the private keys, `inbox.key.sealed`, is sealed by
  Windows (DPAPI) for this computer, so a copy of the folder is useless elsewhere. On
  macOS the server runs as you, with its data in `~/Library/Application
  Support/MoneyKeep/capture-server` (only you can open it), and `inbox.key.sealed` is
  sealed with a key kept in your login keychain (item "MoneyKeep capture server"). On
  Linux the server runs as you and `inbox.key` is a file only you can read.
- **What the administrator helper does**: the Windows permission prompt runs
  MoneyKeep's helper with a plan that MoneyKeep wrote. The helper checks that the
  plan is exactly the one MoneyKeep approved (by its SHA-256) and does only
  MoneyKeep's own things: its firewall rule "MoneyKeep capture (Wi-Fi)" for the
  capture server's Wi-Fi ports and program, removing Windows' Block rules for that
  program, its own data folder, its services, and the tunnel token you set up. A
  plan asking for anything else is refused whole ("Nothing was changed").
- **Pairing can't hold up the server**: a pairing request must send its data within
  10 seconds; at most 4 unpaired requests are handled at once (2 from one phone), on
  their own worker, so paired phones are never kept waiting.
- **The log** (`server.log` in the data folder) holds no tokens, amounts or
  merchants; it is replaced at 5 MB, keeping the 3 before it.
- **What is on the network**: the certificate page (port 14080) serves only the
  certificate page and the public certificate; everything else there redirects to
  HTTPS. The phone app and its API (port 14443) need TLS; the desktop's own API is
  not reachable from the network.
- **A lost phone**: revoke it in MoneyKeep at once (Paired phones > Revoke). Consider
  changing the Capture PIN.
- Your book is never on the server or the phone; the phone receives only the lists
  it needs for its menus (categories, payment methods, recent merchants).

## 11. When to use Cloudflare instead

Use **phone capture over Cloudflare** (Settings > Advanced) when:

- you want the phone to send **from away from home** at once, not when back on the
  Wi-Fi;
- your network keeps devices apart (office or guest Wi-Fi with client isolation) and
  this cannot be changed;
- you already run a Cloudflare Tunnel.

Both can be on at once. A phone uses one of them: the Wi-Fi address and the
Cloudflare address are two separate apps to the phone. For a phone that must send
from away from home, pair it over Cloudflare.

# Advanced: phone capture over Cloudflare

This guide sets up phone capture as `docs/specs/mobile-capture.md` (revision 2)
describes: a small capture server on the computer MoneyKeep runs on, reached from
your phone at one address (for example `https://capture.<your domain>`) through
Cloudflare Tunnel, behind Cloudflare Access. The server serves the phone app and its
API on that address. Your book never leaves your computer; the server only holds
what the phone sends until MoneyKeep collects it.

Written from Cloudflare's public documentation: creating a remotely-managed tunnel
with the API
(<https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/get-started/create-remote-tunnel-api/>),
`cloudflared` downloads
(<https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/downloads/>),
Access service tokens
(<https://developers.cloudflare.com/cloudflare-one/access-controls/service-credentials/service-tokens/>),
Access application tokens and their validation
(<https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/application-token/>,
<https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/validating-json/>)
and the Cloudflare API reference.

```
Phone browser ──HTTPS──> capture.<your domain>   (Cloudflare Access: your email)
                               │  Cloudflare Tunnel; cloudflared connects outward,
                               ▼  no router port is opened
               moneykeep-server on 127.0.0.1:8787  (phone app at /, API at /api/v1)
                               ▲
          MoneyKeep desktop ──┘  directly on this computer (desktop token); and through
                                  the public address with an Access service token for
                                  the status light
```

You need: a domain on Cloudflare, Cloudflare Zero Trust turned on once (choose a team
name; the free plan is enough for one person), and the computer MoneyKeep runs on
staying on while you want the phone to send.

## 0. Building MoneyKeep with its capture server (developers)

Users install MoneyKeep with its setup, which already contains the capture server
with the phone app; skip this section. Developers build everything from one checkout
with:

```sh
cargo xtask release              # dist/MoneyKeep-<version>/: the programs, licences, version.json
cargo xtask release --dry-run    # only lists the steps and files
cargo xtask installer            # Windows: then dist/moneykeep-<version>-windows-x64-setup.exe and SHA256SUMS.txt (needs NSIS)
```

`release` builds the phone app, the server with the phone app inside, the desktop
app, the CLI and `keep-mcp` in release mode, and writes `version.json` with the
**capture version**: a short hash of exactly the sources the server and the phone app
are built from. The desktop compares its own capture version with the running
server's (the server reports it in `/api/v1/health`), so it offers **Update now**
only when the server really changed, and only when the server program beside it came
from the same build. A desktop built on its own (`cargo build -p moneykeep-app`)
says "This copy of MoneyKeep was built without its capture server" instead.

The older two-step way still works: `cargo xtask dist-capture` builds only the phone
app and the server. It installs the `wasm32-unknown-unknown` target and Trunk when they are missing,
builds the phone app, builds the server with the app inside (in its own
`target/capture-build` folder) and puts `moneykeep-server`
(`moneykeep-server.exe` on Windows) next to the MoneyKeep program in
`target/release`, where the setup looks for it. A server built without the phone
app answers every page with "The phone app wasn't built into this server — run
cargo xtask dist-capture".

The service never runs the program in the build folder: setup copies it into its
own **install folder** (Windows: `%ProgramFiles%\MoneyKeep\capture`; Linux:
`~/.local/share/moneykeep/capture`), so rebuilding never touches the running server.
On macOS the server runs from inside the installed MoneyKeep.app
(`Contents/MacOS/moneykeep-server`); a development build outside an app bundle uses
`~/Library/Application Support/MoneyKeep/capture`.

**Updating after a rebuild.** Run `cargo xtask release` (or `dist-capture`) again.
When the capture version changed, Settings > Phone capture shows **The capture server
needs an update** with **Update now**: Windows asks for permission once, then
MoneyKeep stops the service, copies the new program into the install folder (keeping
the old one until the new one answers), starts it and checks it; if the new one does
not start, the old one is put back. You never need to type service commands. Installs made before
the install folder existed (their service runs the program in `target\release`) are
moved by the first Update now (or Update server); until then `dist-capture` sets the locked old file
aside instead of failing.

If the command says the wasm32 target or Trunk could not be installed, install them
yourself (`rustup target add wasm32-unknown-unknown`, `cargo install trunk --locked`)
on a computer with internet access and run it again.

## 1. Guided setup (recommended)

In MoneyKeep turn on Features > Expenses, then open Settings > **Advanced** > **Phone capture over Cloudflare** and
press **Set up phone capture over Cloudflare…**. The dialog walks through eight steps; each has one
button that does the work and shows a check mark or a plain error with **Try again**.
You can close the dialog and continue later where you stopped. **Details** at the
bottom lists every Cloudflare call (method, path, result), never a token.

1. **Before you start.** Create an API token at Cloudflare (**Open Cloudflare token
   page**, then "Create Token" > "Create Custom Token") with these permissions
   (**Copy list** copies them):
   - Account · Cloudflare Tunnel · Edit
   - Account · Access: Apps and Policies · Edit
   - Account · Access: Service Tokens · Edit
   - Account · Access: Organizations, Identity Providers, and Groups · Read
   - Zone · DNS · Edit (for the zone you will use)
   - Zone · Zone · Read
2. **Connect to Cloudflare.** Paste the token and press **Check token**. MoneyKeep
   lists the accounts and domains the token can see (pick one when there are
   several) and names any missing permission. MoneyKeep checks read access here;
   a missing *Edit* right shows up as a Cloudflare error during Install.
3. **Address and sign-in.** The subdomain (default `capture`) and the email
   addresses allowed to sign in on the phone. The address must not already exist in
   your DNS (unless MoneyKeep made it).
4. **Where does your tunnel run?**
   - **Create a new tunnel on this computer** (the default when no `cloudflared`
     service runs here);
   - **Add to my existing tunnel <name>**, then **This computer** or **Another device
     on my network** (a router or server running `cloudflared`). For another
     device, the capture server listens on this computer's LAN address (shown and
     editable), and the tunnel sends requests to `http://<that address>:8787`. Only
     tunnels managed in the Cloudflare dashboard can be changed by MoneyKeep; for a
     tunnel run from a `config.yml`, add the rule yourself (see the manual setup).
5. **Install.** The plan lists what will be created. **Install** asks Windows once
   for permission (UAC) and then:
   - installs `moneykeep-server` as a service that starts with the computer, with
     its data folder (Windows: `C:\ProgramData\MoneyKeep\capture-server`, which only
     the service's own account `NT SERVICE\MoneyKeepCapture`, SYSTEM and
     administrators can open; Linux and macOS: your application data, owner-only),
     and keeps the desktop token in MoneyKeep's secrets (in the book if it has a
     password, otherwise in your computer's credential store);
   - for a new tunnel: creates it, and installs `cloudflared` as a service with the
     tunnel's token. On Windows MoneyKeep runs only the `cloudflared` that
     Cloudflare's installer puts in `C:\Program Files\cloudflared` (or
     `C:\Program Files (x86)\cloudflared`), never one found elsewhere, and only after
     checking that Windows says it is validly signed by Cloudflare, Inc. ("The
     cloudflared program on this computer isn't signed by Cloudflare, so MoneyKeep
     won't run it." otherwise). Without one, the official installer is downloaded
     from Cloudflare's GitHub releases into `C:\ProgramData\MoneyKeep\tmp` (a folder
     only administrators can open), held so nothing can change it, and installed
     only when its signature is Cloudflare's. With `cloudflared` 2025.4.0 or later,
     the tunnel token is kept in a file only administrators can open
     (`C:\ProgramData\MoneyKeep\cloudflared`), and the `cloudflared` service runs with
     `--token-file` (Cloudflare's "Tunnel run parameters"). An older `cloudflared`
     takes the token on its service's command line, where administrators of this
     computer can see it; on Linux `cloudflared service install` puts it in
     the service's settings file, which other people who use the computer may be able
     to read. On macOS the token always goes in a file only you can open, in the
     capture server's data folder (so `cloudflared` 2025.4.0 or later is needed). Update `cloudflared` to keep it off the command line. An existing
     `cloudflared` service for another tunnel that still exists in your account is
     never replaced (choose **Add to my existing tunnel**). If that service runs a
     tunnel that no longer exists in your account (for example you deleted it in the
     dashboard), Install says so and offers **Replace it** (the default) or **Leave
     it**: Replace removes that service and installs `cloudflared` for the new tunnel
     in the same Windows permission prompt; Leave keeps Install unavailable;
   - routes the address to the server in the tunnel (an existing tunnel keeps its
     rules; the new one goes before the catch-all) and creates the proxied DNS
     record;
   - creates the Access application for the address with an Allow policy for your
     emails and a 30-day session (so the phone rarely signs in again), and writes
     its AUD tag and your team domain into `server.toml`;
   - creates an Access service token "MoneyKeep desktop status" with a Service Auth
     policy on the same application. The server accepts this identity only for
     `/api/v1/health` and the desktop API (which still needs the desktop token).
6. **Check.** Runs the status checks and shows the light (see "Status light").
7. **Pair your phone.** Scan the QR code with the phone's camera. It opens
   `https://<address>/#pair=<code>`; the phone signs in with Cloudflare's email code
   once and pairs by itself (the code is also shown, valid for 10 minutes). The part
   after `#` never leaves the phone.
8. **Finish.** **Delete Cloudflare token — each change requires a new token** is
   ticked: MoneyKeep erases its copy of the API token. Revoke it at Cloudflare too
   (the link opens the token page): a token can only delete itself with extra
   permissions, which are not asked for. Untick to keep the token for later
   changes.

On Linux the same steps install the server as a systemd user service; `cloudflared`
must already be installed (Cloudflare's package repository) and its service is
installed with administrator rights (`pkexec`). MoneyKeep looks for it only in
`/usr/local/bin` or `/usr/bin`, never elsewhere on your `PATH`.

On macOS MoneyKeep never downloads `cloudflared`: install Cloudflare's own
`cloudflared` package for macOS, then press **Check again**. MoneyKeep runs it only
from `/usr/local/bin/cloudflared` or `/opt/homebrew/bin/cloudflared` (never your
`PATH`), and only when `codesign` confirms a valid signature from Cloudflare's Apple
team. Nothing needs an administrator: the tunnel runs as your own background agent
(`com.moneykeep.cloudflared`), like the capture server, while you're logged in.

**Later changes.** Settings > Advanced offers **Change address or emails…**,
**Repair Cloudflare settings** (sets the Access application's 30-day session and its
policies again — use it once for setups made before this was the default),
and **Remove phone capture over Cloudflare…**. To pair another phone use Settings >
Phone capture > Paired phones > **Pair a phone…** (the QR code, with the code to type
under "Can't scan?"). Changes ask for an API
token when none is stored. A different address: remove phone capture, then set it up
again. **Remove phone capture over Cloudflare** lists what will go, then uninstalls the services and
deletes, through the API, exactly what setup created (DNS record, the tunnel or only
its rule in an existing tunnel, Access application, policies, service token).

## 2. Status light

A round light left of the Calculator button (and a large one in Settings > Phone
capture) checks every minute while MoneyKeep is open, and on **Check now**:

- **Green** (filled) "Phone capture is working": the server answers on this computer
  and through the internet address, and collecting works.
- **Yellow** ("!"): partly wrong, named in the tooltip: "The internet address isn't
  reaching your computer (tunnel down?)", "Collecting from the server failed: …",
  "Server version differs from the app".
- **Red** ("×") "Phone capture isn't working: the server isn't running", with **Start
  the server** when the service exists but is stopped; or "… the capture server isn't
  installed", with **Install the server again**, when the service is missing but its
  data is still on this computer.
- **Grey** (hollow) "Phone capture is off".

The tooltip also shows the last phone activity, items waiting and the last check.

## 3. Manual setup (fallback)

Use this when you prefer not to give MoneyKeep an API token, or for a tunnel run from
a `config.yml`.

1. **Initialise the server.** `moneykeep-server init --data <folder> --key-file`
   creates `server.toml`, the inbox and `inbox.key`, and prints the desktop token
   once (`rotate-desktop-token` makes a new one). Keep the folder private (`chmod
   700` on Linux/macOS; on Windows only your account, SYSTEM and Administrators).
2. **Run it as a service.**
   - Linux (systemd user unit `~/.config/systemd/user/moneykeep-capture.service`):

     ```ini
     [Unit]
     Description=MoneyKeep phone capture server
     After=network-online.target

     [Service]
     ExecStart=/path/to/moneykeep-server run --data /home/<you>/.moneykeep-capture --key-file
     Restart=on-failure

     [Install]
     WantedBy=default.target
     ```

     then `systemctl --user enable --now moneykeep-capture` (and `loginctl
     enable-linger` to start it at boot). Logs: `journalctl --user -u
     moneykeep-capture`.
   - Windows, from an administrator Command Prompt:
     `sc.exe create MoneyKeepCapture start= auto obj= "NT SERVICE\MoneyKeepCapture" binPath= "\"C:\path\moneykeep-server.exe\" service-run --data \"C:\ProgramData\MoneyKeep\capture-server\" --key-file"`,
     `sc.exe sidtype MoneyKeepCapture unrestricted`, give `NT SERVICE\MoneyKeepCapture`
     Modify on the data folder (and nobody but SYSTEM and Administrators anything else),
     then `sc.exe start MoneyKeepCapture`. The service logs to `server.log` in the
     data folder.
   - macOS: a LaunchAgent running `moneykeep-server service-run --data <folder>
     --key-file` with `RunAtLoad`, `KeepAlive` and `ThrottleInterval` 10, loaded
     with `launchctl bootstrap gui/$(id -u) <plist>`; it logs to `server.log` in the
     data folder.

   Every request that comes through the tunnel must carry a valid Access token,
   the desktop API included; requests from this computer itself don't.
   Logs show method, path template, status and timing only — never tokens, merchants
   or amounts. The server listens on `127.0.0.1:8787`; any other address needs
   `--allow-lan`.
3. **Tunnel.** In Zero Trust > Networks > Tunnels, add a public hostname
   `capture.<your domain>` with service `HTTP`, URL `127.0.0.1:8787` (or
   `<this computer's LAN address>:8787` when `cloudflared` runs on another device,
   with `listen` set to that address and `--allow-lan`). For a `config.yml` tunnel,
   add an ingress rule `hostname: capture.<your domain>`,
   `service: http://127.0.0.1:8787` above the catch-all rule and restart
   `cloudflared`.
4. **Access.** Zero Trust > Access > Applications: one **self-hosted** application for
   `capture.<your domain>` with an **Allow** policy for your email and a session
   duration of **1 month** (720 hours), so the phone rarely needs to sign in again. Optionally
   create a service token, add a **Service Auth** policy for it, and enter its Client
   ID as `access_service_client_id` (used by the status light's internet check; the
   manual setup's light checks only this computer). Copy the application's
   **Application Audience (AUD) tag**.
5. **`server.toml`.**

   ```toml
   listen = "127.0.0.1:8787"
   access_required = true
   access_team_domain = "yourteam.cloudflareaccess.com"
   access_aud = "<the application's AUD tag>"
   access_service_client_id = ""
   max_receipt_mb = 10
   max_receipts_per_expense = 10
   inbox_limit_mb = 1000
   client_url = "https://capture.<your domain>"
   ```

   (`moneykeep-server configure --data <folder> --team-domain … --aud …
   --client-url …` writes these.) Restart the service. Phone requests now need a
   valid Access token for that application; check the computer's clock is right
   (tokens more than a minute out of time are refused, and the log says so).
6. **MoneyKeep.** Settings > Advanced > Connection: server address
   `http://127.0.0.1:8787` (or `http://<LAN address>:8787` with "This network is
   trusted"), the desktop token, **Save**, **Test connection**.
7. **Pair.** **Pair a phone…** shows a QR code to scan and, under "Can't scan?", an
   8-character code for 10 minutes; or open `https://capture.<your domain>` on the
   phone, sign in, enter the code and a name, and tap **Pair**.

## Install the app on the phone

The phone app is an installable web app: it opens like an app, works offline (its
outbox keeps what you enter until there is a connection) and keeps its pairing.

- **Android (Chrome):** the app shows **Install app** on its home screen (or use
  the browser menu > Install app). Open it from the new icon.
- **iPhone/iPad (Safari):** tap Share > **Add to Home Screen**, then open the app
  from the icon and pair *inside it*: the installed app has its own storage, separate
  from Safari's.
- The app asks the browser to keep its storage; the menu's **About this phone**
  shows "Storage: kept" or "Storage: may be cleared by the browser".
- The pairing ends only when you choose **Sign out** or when the phone is revoked in
  MoneyKeep. When Cloudflare's sign-in has expired, the app shows **Sign in to
  Cloudflare again** with a **Sign in** button: it opens the app's start page, which
  Cloudflare sends to its sign-in and then back to the app; the pairing stays.
- The app's own files are fetched from the network first and kept for offline use,
  so a new version reaches the phone the next time it opens online; offline it opens
  from the kept copy.

On iPhone, set Settings > Camera > Formats to **Most Compatible** so photos are saved
as JPEG.

## Everyday use

- Take a photo or enter an expense on the phone; it is saved on the phone first and
  sent when there is a connection. While the computer is off the app still opens and
  keeps working (see "When your computer is off" in Everyday use above). The Sent list shows "Waiting for your computer"
  until MoneyKeep collects it, then "Collected on <date>".
- MoneyKeep collects when the book opens and every 15 minutes while it is open
  (**Collect automatically**; **Collect now** at once). Collected items appear in
  Expenses with a "From phone" badge, and Home shows "N expenses from your phone to
  review". Photos sent without fields land in the Expenses Inbox tab.

## A lost phone, and rotating tokens

- **Lost phone:** Settings > Advanced > Paired phones > **Revoke**. Its next
  request is refused. Also end its Access session (Zero Trust > My Team > Users >
  the user > Revoke sessions).
- **Desktop token:** run the setup's Install again, or
  `moneykeep-server rotate-desktop-token --data <folder> --key-file` and paste the
  new token in Settings > Advanced.
- **Service token:** remove phone capture and set it up again, or create a new token
  in Access, add it to the Service Auth policy and delete the old one.

## Remove phone capture

Settings > Advanced > **Remove phone capture over Cloudflare…** takes away what
setup made, and nothing else:

- the Access application for the address, its two policies ("MoneyKeep phone
  capture - allowed people" and "MoneyKeep desktop status - service token") and the
  service token "MoneyKeep desktop status";
- the DNS record for the address (only the one MoneyKeep made);
- the tunnel, when setup created it; for an existing tunnel only MoneyKeep's rule
  for the address (the other rules stay, in order, before the catch-all
  `http_status:404`);
- the capture server service and, when setup installed it, the `cloudflared`
  service on this computer (one Windows permission prompt).

If something cannot be removed, the list names it with the reason and where to
remove it by hand, and the rest is still removed; press **Retry undo** to try again.
Phone capture is not shown as removed while anything is left.

**Cleaning up by hand.** If you deleted things in the Cloudflare dashboard first (the
tunnel, the application), remove phone capture in MoneyKeep anyway: what is already
gone counts as removed. To remove a leftover `cloudflared` service yourself, open a
terminal as administrator and run `cloudflared service uninstall`, or
`sc.exe stop cloudflared` and then `sc.exe delete cloudflared`. On Linux:
`sudo cloudflared service uninstall`. A later setup can also replace the service of
a deleted tunnel for you (Install > **Replace it**).

After `sc.exe delete cloudflared`, Windows keeps cloudflared's event-log entry, and
the next `cloudflared service install` fails with "Cannot install event logger … registry
key already exists". Remove it too (administrator terminal):
`reg delete "HKLM\SYSTEM\CurrentControlSet\Services\EventLog\Application\Cloudflared" /f`.
MoneyKeep's setup, Replace it, Remove and Start over remove it for you.

**Start over.** Settings > Phone capture > **Start over…** (also offered as "Stuck?
Start over…" when a setup step fails) removes everything phone capture set up: in
Cloudflare only what MoneyKeep's setup made (by id), on this computer the capture
server service, the `cloudflared` service when MoneyKeep installed it or its tunnel
no longer exists (one for a live tunnel or in another Cloudflare account is kept), the
Wi-Fi firewall rule and the capture server's data folder, and in MoneyKeep the saved
Cloudflare token, the Capture PIN and the setup. It shows the list first; type
`START OVER` to go ahead. Collect waiting phone expenses first (the dialog offers
**Collect now**). Books and backups are never touched. When Cloudflare can't be
reached, the rest still runs and the list says what to delete in the dashboard.

## What to back up

Nothing essential: the inbox is temporary, and everything collected is in your book
(back that up as usual). If the server's data folder is lost, run the setup's Install
again (or `init` by hand) and pair the phone again. Anything not yet collected would
be lost; the phone keeps whatever it had not sent.

## Troubleshooting

- Phone says "Sign in to Cloudflare again": the Access session ended; tap **Sign
  in** (or open the app's address again) to sign in.
- Phone says "The capture server did not accept this Cloudflare sign-in": the
  server's Access settings (usually `access_aud` in `server.toml`) do not match the
  Access application. In MoneyKeep, Settings → Phone capture shows the reason;
  press **Check**, which fixes `server.toml` and restarts the server (it does not
  replace the server program)
  (`moneykeep-server set-aud --data DIR --aud TAG` does the same by hand; restart
  the service after it).
- Phone says "This phone was disconnected — pair again to send": it was revoked;
  pair it again (its unsent items are kept).
- Upload refused with "Storage full": open MoneyKeep to collect, or raise
  `inbox_limit_mb`.
- Yellow light "The internet address isn't reaching your computer": check that the
  `cloudflared` service runs (Windows Services, `systemctl status cloudflared`) and
  the tunnel shows Healthy in the Cloudflare dashboard.
- Red light: press **Start the server** (or **Install the server again**), or start the "MoneyKeep phone capture server"
  service yourself; on Windows its log is `server.log` in
  `C:\ProgramData\MoneyKeep\capture-server` (open it as administrator).
- Install says Windows did not give permission: press **Try again** and accept the
  prompt.
- Install says "A cloudflared service on this computer runs a tunnel that no longer
  exists in your Cloudflare account": keep **Replace it** and press **Install**, or
  see "Remove phone capture" to remove that service by hand.
