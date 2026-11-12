# Opening balances

Your books need to know what each account held on the day you start using
MoneyKeep. That amount is the account's opening balance. You don't need to find
it in an old statement: give one balance you trust and MoneyKeep works back from
it.

## Working backwards

Pick a balance you trust and its date: the balance your bank feed reports, the
ending balance of a statement, or what your bank's website shows today. MoneyKeep
takes the transactions already in the account after your start date and up to
that date, and subtracts them. What is left is the opening balance.

- Transactions dated after the balance's date are not counted.
- Pending transactions from a bank feed are counted only if the account's feed
  setting includes pending transactions. The wizard says which applies.
- For a credit card, loan or other debt, type what you owe as a positive number.
  If a card owes you money, turn on "The card owes me".

## Choosing a start date

- **Keep everything imported** (the usual choice): your books start the day before
  the account's earliest transaction, and every transaction stays.
- **Start at a statement**: give a statement's ending date and ending balance.
  Your books start on that date. Transactions on or before it that are not
  reconciled are removed, because the statement balance already includes them.
  You can untick any you want to keep. Removed bank feed transactions are
  remembered, so the next sync does not bring them back.

If the account has no transactions yet, the opening balance is simply its
balance today.

## The other side

Every amount in MoneyKeep comes from somewhere. An opening balance comes from the
**Opening Balances** account: it stands for everything you had before you started,
and it keeps your income and spending reports clean.

- In a business book you can choose **Owner's Equity** instead, for money you put
  into the business.
- A loan for something you own, such as a house or a car, can add that thing as
  an asset with its own opening balance, so your net worth counts both.
- You can choose any other account from the full list.

## The check

Before saving, the wizard shows the account's balance on the balance's date with
the new opening balance and compares it with the balance you trust. If they
differ, it lists likely causes: pending transactions, possible duplicates (the
same amount within 3 days), transactions dated after the balance's date,
transactions you kept before a statement start, or a balance for a different
date.

If the change would make a finished reconciliation stop adding up, the wizard
says so. You can pick an earlier start date, or go ahead: the reconciliation is
then marked to check.

## Investment accounts

For a brokerage account the wizard sets up the cash the same way, then the
holdings on your start date and their cost basis.

- Shares on the start date are worked out from the shares you hold today and the
  purchases, sales and splits recorded after the start date.
- Cost basis can be given as exact lots, as one total (or an average cost per
  share), or as **I don't know yet**. Then the holding is recorded at its market
  value on the start date and marked **Basis needed** until you supply it. Look
  for it on your broker's cost basis or unrealized gain page, or on last year's
  1099-B.
- A retirement account (IRA, 401(k), Roth and similar) does not need cost basis:
  holdings are recorded at market value. You can still enter it if you want, for
  example for after-tax contributions.

## The setup checklist

The checklist lists every account with its status: Done, Not set, Basis needed or
Started from zero, and shows how far along you are. Open it from Home (Attention)
or from Accounts → … → Opening balances…. If an account really started at zero,
choose "This account started from zero" in its … menu.

## Nothing is locked

An opening balance is an ordinary transaction labelled "Opening balance". It
appears in the register and can be edited there. Run the wizard again at any time
to change the amount, the date or the other side: it changes the same transaction
and never adds a second one. Every save can be undone.
