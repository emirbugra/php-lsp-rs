<h1>Kullanıcılar</h1>
<?php foreach ($users as $user): ?>
    <p><?= $user->getName() ?> — <?= $config->baseURL ?></p>
<?php endforeach ?>
